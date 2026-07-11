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

        self.kick_mason_seed_if_needed();
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

    /// Build the initial `ExtensionEntry` list from the bundled MCP registry
    /// merged with any Mason-cached LSP entries, joined against the local
    /// `installed.json` so already-installed servers render with their
    /// `Installed { version }` status. Also populates
    /// `renderer.bundled_manifests` so the install/uninstall dispatcher
    /// can re-resolve a full manifest from an `ExtensionEntry` id.
    fn load_bundled_extension_entries(&mut self) -> Vec<ExtensionEntry> {
        let notes_manifest = neoism_notes_mcp_manifest();
        let memory_manifest = neoism_memory_mcp_manifest();
        let builtins = vec![notes_manifest.clone(), memory_manifest.clone()];
        let installed = ensure_builtin_mcp_installed(&builtins);

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

        // Mason LSPs: load from the cache if present (first launch will
        // miss; `kick_mason_seed_if_needed` warms it asynchronously and
        // the next open picks them up). Filter to entries whose
        // categories mention "LSP" so the panel's Language Servers tab
        // surfaces just them. Tag with "Language Server" for the same
        // tab-filter substring-match reason.
        // Mason: keep packages tagged as LSP / Formatter / Linter so the
        // panel can split them across tabs. The "Tag" insertion below
        // is a safety net — Mason's own category strings already cover
        // most of the substrings the tabs match on, but a couple of
        // entries categorise themselves only as "Compiler" / "Runtime"
        // alongside one of the three; tagging makes the tab filter
        // deterministic regardless of which category Mason picked.
        let mason_path = neoism_extensions::mason::mason_cache_path();
        match neoism_extensions::load_mason_registry(&mason_path) {
            Ok(reg) => {
                let translated = neoism_extensions::translate_registry(&reg);
                let mut tool_manifests: Vec<ExtensionManifest> = Vec::new();
                let mut lsp_count = 0usize;
                let mut fmt_count = 0usize;
                let mut lint_count = 0usize;
                for mut m in translated {
                    let is_lsp =
                        m.categories.iter().any(|c| c.eq_ignore_ascii_case("LSP"));
                    let is_fmt = m
                        .categories
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case("Formatter"));
                    let is_lint = m
                        .categories
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case("Linter"));
                    if !(is_lsp || is_fmt || is_lint) {
                        continue;
                    }
                    // Ensure a tab-friendly tag is present so
                    // `matches_tab` resolves cleanly. Multi-purpose
                    // packages (e.g. ruff = LSP + Formatter + Linter)
                    // appear in every tab they qualify for.
                    if is_lsp
                        && !m.categories.iter().any(|c| {
                            c.eq_ignore_ascii_case("Language Server")
                                || c.eq_ignore_ascii_case("LSP")
                        })
                    {
                        m.categories.insert(0, "Language Server".to_string());
                    }
                    if is_lsp {
                        lsp_count += 1;
                    }
                    if is_fmt {
                        fmt_count += 1;
                    }
                    if is_lint {
                        lint_count += 1;
                    }
                    tool_manifests.push(m);
                }
                tracing::info!(
                    target: "neoism::extensions",
                    mason_packages = reg.len(),
                    lsp = lsp_count,
                    formatters = fmt_count,
                    linters = lint_count,
                    "loaded mason registry"
                );
                mcp_manifests.extend(tool_manifests);
            }
            Err(err) => {
                tracing::warn!(
                    target: "neoism::extensions",
                    path = %mason_path.display(),
                    ?err,
                    "mason registry load failed; tool entries will appear after background fetch"
                );
            }
        }

        let python_kernel_manifest = neoism_python_kernel_manifest();
        let rust_kernel_manifest = evcxr_jupyter_kernel_manifest();
        let syntax_manifests = treesitter_parser_manifests();
        let mut manifests = mcp_manifests;
        manifests.extend(syntax_manifests);
        manifests.insert(0, rust_kernel_manifest);
        manifests.insert(0, python_kernel_manifest);
        manifests.insert(0, memory_manifest);
        manifests.insert(0, notes_manifest);

        // Populate the manifest cache keyed by id BEFORE consuming the
        // vec into entries — needed by the install/uninstall dispatch.
        self.renderer.bundled_manifests = manifests
            .iter()
            .map(|m| (m.id.clone(), m.clone()))
            .collect();

        manifests
            .into_iter()
            .map(|m| {
                let installed_version =
                    installed.get(&m.id).map(|e| e.version.clone()).or_else(|| {
                        treesitter_lang_from_extension_id(&m.id)
                            .filter(|lang| treesitter_parser_installed(lang))
                            .map(|_| m.version.clone())
                    });
                extension_manifest_to_entry(m, installed_version)
            })
            .collect()
    }

    /// First-time-open trigger to background-fetch the Mason registry
    /// snapshot. We don't block the UI on it — on cache miss the panel
    /// just shows the bundled MCP entries this session and LSPs land on
    /// next open. Subsequent opens hit `ensure_cached_mason_registry`'s
    /// 24h freshness check and short-circuit immediately.
    fn kick_mason_seed_if_needed(&mut self) {
        if self.renderer.mason_seeded {
            return;
        }
        self.renderer.mason_seeded = true;
        // Detached: the result hits disk; the per-frame pump in
        // `render_neoism_extensions_panels` watches `MASON_CACHE_FRESH`
        // and re-seeds the visible pane's entries once this lands.
        ext_runtime_handle().spawn(async move {
            let _ = neoism_extensions::ensure_cached_mason_registry().await;
            MASON_CACHE_FRESH.store(true, Ordering::Release);
        });
    }

    /// Called from `render_neoism_extensions_panels` once per frame. If
    /// the background Mason seed has reported a fresh cache since the
    /// last call, rebuild the Extensions pane's entries (preserving
    /// install state via `installed.json`) so LSP rows appear without
    /// the user closing/reopening the tab.
    pub(crate) fn drain_mason_cache_refresh(&mut self) {
        if !MASON_CACHE_FRESH.swap(false, Ordering::AcqRel) {
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

    /// Drain the install tracker, applying the latest progress to each
    /// row and finalising completed jobs (write `installed.json`, agent
    /// config, refresh nvim's managed bin map, flip row status). Called
    /// per-frame BEFORE rendering so the visible bar tracks the runner.
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
                        job.last_percent = percent;
                        job.last_status = status;
                    }
                    ProgressEvent::Downloading { bytes, total } => {
                        if let Some(t) = total {
                            if t > 0 {
                                job.last_percent = ((bytes * 75) / t).min(75) as u8 + 5;
                            }
                        }
                        job.last_status = format!("downloading ({bytes} bytes)");
                    }
                    ProgressEvent::Started => {
                        job.last_status = "starting…".to_string();
                    }
                    ProgressEvent::Extracting => {
                        job.last_status = "extracting".to_string();
                        job.last_percent = job.last_percent.max(85);
                    }
                    ProgressEvent::Linking => {
                        job.last_status = "linking".to_string();
                        job.last_percent = job.last_percent.max(95);
                    }
                    ProgressEvent::Done => {
                        job.last_percent = 100;
                    }
                    ProgressEvent::Failed { message } => {
                        job.last_status = format!("failed: {message}");
                    }
                }
                if matches!(job.source, InstallSource::PythonKernelModal { .. }) {
                    self.renderer
                        .modal
                        .open(neoism_ui::widgets::modal::ModalSpec {
                            title: "Installing Python Kernel".to_string(),
                            body: job.last_status.clone(),
                            meta: format!("{}% complete", job.last_percent),
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

            if job.join_handle.is_finished() {
                completed.push(id.clone());
            }
        }

        // Second pass: finalise completed jobs. We have to remove them
        // out-of-band so the borrow on `in_flight` is released before
        // we mutate notifications / nvim broadcast paths.
        if completed.is_empty() {
            return;
        }
        let mut retry_treesitter_syntax = false;
        for id in completed {
            let Some(job) = self.renderer.install_tracker.in_flight.remove(&id) else {
                continue;
            };
            let source = job.source.clone();
            if let InstallSource::TreeSitterParser { lang } = &source {
                self.treesitter_installing.remove(lang);
            }
            // futures::executor::block_on is safe here: the JoinHandle
            // already reports finished, so the underlying task completed
            // and the await is just a synchronous unwrap of its result.
            let outcome = futures::executor::block_on(job.join_handle);
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
                        }
                    }
                    self.finalize_install_success(&id, &source);
                    // A newly-installed LSP/formatter binary needs no nvim
                    // push anymore: the Rust LSP engine re-reads the managed
                    // bin map on every `status()`. Only tree-sitter parsers
                    // still need an nvim-side syntax retry.
                    if matches!(source, InstallSource::TreeSitterParser { .. }) {
                        retry_treesitter_syntax = true;
                    }
                    // A newly-installed LSP server needs no nvim re-attach: the
                    // Rust engine discovers it from the managed bin map on its
                    // next status()/completion query.
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
        if retry_treesitter_syntax {
            self.retry_treesitter_syntax_in_nvim();
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
                            "Neoism installed `{}` and asked embedded nvim to start the LSP for the current buffer again.",
                            server
                        ),
                        meta: "Use :LspInfo to inspect active clients.".to_string(),
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
            InstallSource::TreeSitterParser { lang } => {
                let display = crate::neoism::ide_tools::treesitter_install_spec(lang)
                    .map(|spec| spec.display_name)
                    .unwrap_or(lang);
                self.renderer.notifications.push(
                    format!("Installed {} syntax parser", display),
                    NotificationLevel::Info,
                );
            }
        }
    }

    pub(crate) fn retry_treesitter_syntax_in_nvim(&mut self) {
        let cmd = neoism_backend::performer::nvim::vim_treesitter_retry_command();
        for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(editor) = item.context().editor.as_ref() {
                    editor.command(cmd.clone());
                }
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
            InstallSource::TreeSitterParser { lang } => {
                let display = crate::neoism::ide_tools::treesitter_install_spec(lang)
                    .map(|spec| spec.display_name)
                    .unwrap_or(lang);
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: format!("Could Not Install {display} Syntax"),
                        body: err.to_string(),
                        meta: "Install git/tree-sitter/cc if missing, then retry."
                            .to_string(),
                        input: None,
                        buttons: vec![
                        neoism_ui::widgets::modal::ModalButton::new(
                            "Retry Install",
                            "Enter",
                            neoism_ui::widgets::modal::ModalAction::InstallTreesitter {
                                lang: lang.clone(),
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
