use super::*;
use std::path::{Path, PathBuf};

impl Screen<'_> {
    pub(crate) fn run_notebook_cell(&mut self, cell_index: usize) -> bool {
        let Some((path, job)) =
            self.context_manager
                .current_mut()
                .notebook
                .as_mut()
                .map(|notebook| {
                    let path = notebook.path.clone();
                    let job = notebook.prepare_cell_execution(cell_index);
                    (path, job)
                })
        else {
            return false;
        };

        let job = match job {
            Ok(job) => job,
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Notebook cell failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                return true;
            }
        };

        let rx = self.notebook_runtime.submit(job);
        self.pending_notebook_executions.push(rx);
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.notifications.push(
            format!("Running notebook cell {}", cell_index + 1),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn retry_notebook_cell_after_python_kernel_install(
        &mut self,
        path: &Path,
        cell_index: usize,
    ) -> Result<(), String> {
        let mut job = Err(format!("Notebook {} is no longer open", path.display()));
        'find_notebook: for grid in self.context_manager.contexts_mut() {
            for item in grid.contexts_mut().values_mut() {
                let context = item.context_mut();
                let Some(notebook) = context.notebook.as_mut() else {
                    continue;
                };
                if notebook.path == path {
                    job = notebook.prepare_cell_execution(cell_index);
                    break 'find_notebook;
                }
            }
        }

        let job = job?;
        let path = job.path.clone();
        let rx = self.notebook_runtime.submit(job);
        self.pending_notebook_executions.push(rx);
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.notifications.push(
            format!("Retrying notebook cell {}", cell_index + 1),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        Ok(())
    }

    pub(crate) fn run_all_notebook_cells(&mut self) -> bool {
        let Some((path, jobs)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let jobs = notebook.prepare_all_cell_executions();
                (path, jobs)
            })
        else {
            return false;
        };

        let jobs = match jobs {
            Ok(jobs) => jobs,
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Notebook run all failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                return true;
            }
        };

        let count = jobs.len();
        for job in jobs {
            let rx = self.notebook_runtime.submit(job);
            self.pending_notebook_executions.push(rx);
        }
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.notifications.push(
            format!("Running {count} notebook cells"),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn run_current_and_below_notebook_cells(&mut self) -> bool {
        let cell_index = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .and_then(|notebook| notebook.current_cell_index())
            .unwrap_or(0);
        self.run_notebook_cell_and_below_from(cell_index)
    }

    pub(crate) fn run_notebook_cell_and_below_from(&mut self, cell_index: usize) -> bool {
        let Some((path, jobs)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let jobs = notebook.prepare_cell_and_below_executions_from(cell_index);
                (path, jobs)
            })
        else {
            return false;
        };

        let jobs = match jobs {
            Ok(jobs) => jobs,
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Notebook run below failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                return true;
            }
        };

        let count = jobs.len();
        for job in jobs {
            let rx = self.notebook_runtime.submit(job);
            self.pending_notebook_executions.push(rx);
        }
        self.sync_markdown_tab_modified(&path, true);
        self.renderer.notifications.push(
            format!("Running {count} notebook cells"),
            neoism_ui::panels::notifications::NotificationLevel::Info,
        );
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn insert_notebook_code_cell_above(&mut self) -> bool {
        self.insert_current_notebook_cell(
            neoism_ui::editor::notebook::NotebookCellType::Code,
            false,
        )
    }

    pub(crate) fn insert_notebook_code_cell_below(&mut self) -> bool {
        self.insert_current_notebook_cell(
            neoism_ui::editor::notebook::NotebookCellType::Code,
            true,
        )
    }

    pub(crate) fn insert_notebook_markdown_cell_above(&mut self) -> bool {
        self.insert_current_notebook_cell(
            neoism_ui::editor::notebook::NotebookCellType::Markdown,
            false,
        )
    }

    pub(crate) fn insert_notebook_markdown_cell_below(&mut self) -> bool {
        self.insert_current_notebook_cell(
            neoism_ui::editor::notebook::NotebookCellType::Markdown,
            true,
        )
    }

    fn insert_current_notebook_cell(
        &mut self,
        kind: neoism_ui::editor::notebook::NotebookCellType,
        below: bool,
    ) -> bool {
        let Some((path, result)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let result = if below {
                    notebook.insert_cell_below(kind)
                } else {
                    notebook.insert_cell_above(kind)
                };
                (path, result)
            })
        else {
            return false;
        };

        match result {
            Ok(index) => {
                self.sync_markdown_tab_modified(&path, true);
                self.flush_current_notebook_crdt();
                let kind = match kind {
                    neoism_ui::editor::notebook::NotebookCellType::Code => "code",
                    neoism_ui::editor::notebook::NotebookCellType::Markdown => "markdown",
                    neoism_ui::editor::notebook::NotebookCellType::Raw => "raw",
                };
                let direction = if below { "below" } else { "above" };
                self.renderer.notifications.push(
                    format!("Inserted {kind} cell {direction} as cell {}", index + 1),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Insert notebook cell failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn delete_current_notebook_cell(&mut self) -> bool {
        self.apply_current_notebook_structure_edit(
            |notebook| notebook.delete_current_cell(),
            |index| format!("Deleted notebook cell {}", index + 1),
            "Delete notebook cell failed",
        )
    }

    pub(crate) fn move_current_notebook_cell_up(&mut self) -> bool {
        self.apply_current_notebook_structure_edit(
            |notebook| notebook.move_current_cell_up(),
            |index| format!("Moved notebook cell to {}", index + 1),
            "Move notebook cell failed",
        )
    }

    pub(crate) fn move_current_notebook_cell_down(&mut self) -> bool {
        self.apply_current_notebook_structure_edit(
            |notebook| notebook.move_current_cell_down(),
            |index| format!("Moved notebook cell to {}", index + 1),
            "Move notebook cell failed",
        )
    }

    fn apply_current_notebook_structure_edit(
        &mut self,
        edit: impl FnOnce(
            &mut neoism_ui::editor::notebook::NotebookPane,
        ) -> Result<usize, String>,
        success_message: impl FnOnce(usize) -> String,
        error_prefix: &str,
    ) -> bool {
        let Some((path, result)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let result = edit(notebook);
                (path, result)
            })
        else {
            return false;
        };

        match result {
            Ok(index) => {
                self.sync_markdown_tab_modified(&path, true);
                self.flush_current_notebook_crdt();
                self.renderer.notifications.push(
                    success_message(index),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("{error_prefix}: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn clear_current_notebook_outputs(&mut self) -> bool {
        let Some((path, result)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let result = notebook.clear_all_outputs();
                (path, result)
            })
        else {
            return false;
        };

        match result {
            Ok(cleared) => {
                self.sync_markdown_tab_modified(&path, true);
                self.flush_current_notebook_crdt();
                self.renderer.notifications.push(
                    format!("Cleared outputs from {cleared} notebook cells"),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.sync_markdown_tab_modified(&path, true);
                self.renderer.notifications.push(
                    format!("Clear notebook outputs failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn clear_current_notebook_cell_output(&mut self) -> bool {
        let cell_index = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .and_then(|notebook| notebook.current_cell_index())
            .unwrap_or(0);
        self.clear_notebook_cell_output(cell_index)
    }

    pub(crate) fn clear_notebook_cell_output(&mut self, cell_index: usize) -> bool {
        let Some((path, result)) = self
            .context_manager
            .current_mut()
            .notebook
            .as_mut()
            .map(|notebook| {
                let path = notebook.path.clone();
                let result = notebook.clear_output_at(cell_index);
                (path, result)
            })
        else {
            return false;
        };

        match result {
            Ok(index) => {
                self.sync_markdown_tab_modified(&path, true);
                self.flush_current_notebook_crdt();
                self.renderer.notifications.push(
                    format!("Cleared output from notebook cell {}", index + 1),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
            }
            Err(err) => {
                self.renderer.notifications.push(
                    format!("Clear notebook cell output failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn interrupt_current_notebook_kernel(&mut self) -> bool {
        let Some((path, running)) = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .map(|notebook| (notebook.path.clone(), notebook.has_running_cells()))
        else {
            return false;
        };

        if !running {
            self.renderer.notifications.push(
                "No running notebook cell to interrupt".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
        } else {
            match self.notebook_runtime.interrupt_kernel(&path) {
                Ok(()) => self.renderer.notifications.push(
                    "Interrupt sent to notebook kernel".to_string(),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                ),
                Err(err) => self.renderer.notifications.push(
                    format!("Notebook interrupt failed: {err}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                ),
            }
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn restart_current_notebook_kernel(&mut self) -> bool {
        let Some((path, running)) = self
            .context_manager
            .current()
            .notebook
            .as_ref()
            .map(|notebook| (notebook.path.clone(), notebook.has_running_cells()))
        else {
            return false;
        };

        if running {
            self.renderer.notifications.push(
                "Wait for running notebook cells to finish before restarting the kernel"
                    .to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
        } else if self.notebook_runtime.restart_kernel(path) {
            self.renderer.notifications.push(
                "Notebook kernel will restart on the next run".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
        } else {
            self.renderer.notifications.push(
                "Notebook runtime is not available".to_string(),
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
        true
    }

    pub(crate) fn handle_notebook_chrome_click(&mut self) -> bool {
        if self.context_manager.current().notebook.is_none() {
            return false;
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let chrome_top = self.island_chrome_top();
        let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
        let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
            &self.context_manager,
            scale_factor,
            logical_width,
        );
        let crumbs_y = if self.renderer.buffer_tabs.is_visible() {
            chrome_top + self.renderer.buffer_tabs.height()
        } else {
            chrome_top
        };
        let row_h = self.renderer.breadcrumbs.height();
        let in_row = mouse_x >= strip_left
            && mouse_x < strip_left + strip_width
            && mouse_y >= crumbs_y
            && mouse_y < crumbs_y + row_h;
        if !in_row {
            return false;
        }

        if self
            .renderer
            .breadcrumbs
            .kernel_selector_at(mouse_x, mouse_y)
        {
            if self.renderer.context_menu.is_notebook_kernel() {
                self.renderer.context_menu.close();
                self.mark_dirty();
                return true;
            }
            return self.open_current_notebook_kernel_menu(mouse_x, crumbs_y + row_h);
        }

        let Some(action) = self.renderer.breadcrumbs.action_at(mouse_x, mouse_y) else {
            return true;
        };
        match action {
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::RunNotebookCell => {
                self.run_current_notebook_cell()
            }
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::RunNotebookCellAndBelow => {
                self.run_current_and_below_notebook_cells()
            }
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::RunAllNotebookCells => {
                self.run_all_notebook_cells()
            }
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::ClearNotebookCellOutput => {
                self.clear_current_notebook_cell_output()
            }
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::ClearNotebookOutputs => {
                self.clear_current_notebook_outputs()
            }
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::InterruptNotebookKernel => {
                self.interrupt_current_notebook_kernel()
            }
            neoism_ui::panels::breadcrumbs::BreadcrumbAction::RestartNotebookKernel => {
                self.restart_current_notebook_kernel()
            }
        }
    }

    fn open_current_notebook_kernel_menu(&mut self, x: f32, y: f32) -> bool {
        let Some(notebook) = self.context_manager.current().notebook.as_ref() else {
            return false;
        };
        let current_name = notebook
            .kernel_name()
            .unwrap_or_else(|| "python3".to_string());
        let current_label = notebook.kernel_display_label();
        let mut kernels = crate::notebook_runtime::list_available_kernels();
        if !kernels.iter().any(|kernel| kernel.name == current_name) {
            kernels.insert(
                0,
                crate::notebook_runtime::AvailableNotebookKernel {
                    name: current_name.clone(),
                    display_name: current_label.clone(),
                    language: "python".to_string(),
                },
            );
        }

        let items = kernels
            .into_iter()
            .map(|kernel| {
                let selected = kernel.name == current_name;
                let hint = if selected {
                    "Current".to_string()
                } else {
                    kernel.name.clone()
                };
                neoism_ui::panels::context_menu::ContextMenuItem::new(
                    kernel.display_name.clone(),
                    hint,
                    neoism_ui::panels::context_menu::ContextMenuAction::Notebook(
                        neoism_ui::panels::context_menu::NotebookContextAction::SelectKernel {
                            name: kernel.name,
                            display_name: kernel.display_name,
                            language: kernel.language,
                        },
                    ),
                )
            })
            .collect::<Vec<_>>();

        let scale_factor = self.sugarloaf.scale_factor();
        let window_size = self.sugarloaf.window_size();
        let logical_width = window_size.width as f32 / scale_factor;
        let logical_height = window_size.height as f32 / scale_factor;
        self.renderer.context_menu.open_notebook_kernel(
            "Kernel",
            items,
            x,
            y + 4.0,
            logical_width,
            logical_height,
        );
        self.mark_dirty();
        true
    }

    pub(crate) fn select_current_notebook_kernel(
        &mut self,
        name: String,
        display_name: String,
        language: String,
    ) -> bool {
        let result = {
            let Some(notebook) = self.context_manager.current_mut().notebook.as_mut()
            else {
                return false;
            };
            let path = notebook.path.clone();
            match notebook.set_kernel_spec(&name, &display_name, &language) {
                Ok(changed) => match notebook.save() {
                    Ok(()) => Ok((path, changed)),
                    Err(err) => Err(format!("Could not save notebook kernel: {err}")),
                },
                Err(err) => Err(err),
            }
        };

        match result {
            Ok((path, changed)) => {
                if changed {
                    self.notebook_runtime.shutdown_kernel(path.clone());
                }
                self.sync_markdown_tab_modified(&path, false);
                self.renderer.notifications.push(
                    format!("Notebook kernel set to {display_name}"),
                    neoism_ui::panels::notifications::NotificationLevel::Info,
                );
                self.mark_dirty();
                true
            }
            Err(err) => {
                self.renderer.notifications.push(
                    err,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                self.mark_dirty();
                false
            }
        }
    }

    pub(crate) fn handle_notebook_chrome_hover(&mut self) -> bool {
        if self.context_manager.current().notebook.is_none() {
            return self.renderer.breadcrumbs.clear_notebook_hover();
        }

        let scale_factor = self.sugarloaf.scale_factor();
        let (mouse_x, mouse_y) = self.mouse_logical_for_hit_test();
        let chrome_top = self.island_chrome_top();
        let logical_width = self.sugarloaf.window_size().width as f32 / scale_factor;
        let (strip_left, strip_width) = self.renderer.workspace_strip_bounds(
            &self.context_manager,
            scale_factor,
            logical_width,
        );
        let crumbs_y = if self.renderer.buffer_tabs.is_visible() {
            chrome_top + self.renderer.buffer_tabs.height()
        } else {
            chrome_top
        };
        let row_h = self.renderer.breadcrumbs.height();
        let in_row = mouse_x >= strip_left
            && mouse_x < strip_left + strip_width
            && mouse_y >= crumbs_y
            && mouse_y < crumbs_y + row_h;
        if !in_row {
            return self.renderer.breadcrumbs.clear_notebook_hover();
        }

        let action_changed = self
            .renderer
            .breadcrumbs
            .set_action_hover_at(mouse_x, mouse_y);
        let kernel_changed = self
            .renderer
            .breadcrumbs
            .set_kernel_hover_at(mouse_x, mouse_y);
        action_changed | kernel_changed
    }

    pub(crate) fn notebook_chrome_action_hovered(&self) -> bool {
        self.renderer.breadcrumbs.action_hovered()
            || self.renderer.breadcrumbs.kernel_hovered()
    }

    pub(crate) fn poll_notebook_executions(&mut self) -> bool {
        const NOTEBOOK_EXECUTION_EVENT_BUDGET: usize = 48;
        const NOTEBOOK_EXECUTION_POLL_BUDGET: std::time::Duration =
            std::time::Duration::from_millis(6);

        use std::collections::HashSet;

        let mut dirty = false;
        let mut pending = Vec::new();
        let mut events = Vec::new();
        let poll_started = std::time::Instant::now();
        let mut budget_exhausted = false;
        let mut disconnected_receivers = 0usize;
        for rx in self.pending_notebook_executions.drain(..) {
            if budget_exhausted {
                pending.push(rx);
                continue;
            }
            let mut keep_pending = true;
            loop {
                if events.len() >= NOTEBOOK_EXECUTION_EVENT_BUDGET
                    || poll_started.elapsed() >= NOTEBOOK_EXECUTION_POLL_BUDGET
                {
                    budget_exhausted = true;
                    break;
                }
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        keep_pending = false;
                        disconnected_receivers = disconnected_receivers.saturating_add(1);
                        dirty = true;
                        break;
                    }
                }
            }
            if keep_pending {
                pending.push(rx);
            }
        }
        self.pending_notebook_executions = pending;
        let event_count = events.len();
        let mut output_events = 0usize;
        let mut display_events = 0usize;
        let mut finished_events = 0usize;
        for (_, event) in &events {
            match event {
                neoism_ui::editor::notebook::NotebookExecutionEvent::Output(_) => {
                    output_events = output_events.saturating_add(1);
                }
                neoism_ui::editor::notebook::NotebookExecutionEvent::DisplayUpdate(_) => {
                    display_events = display_events.saturating_add(1);
                }
                neoism_ui::editor::notebook::NotebookExecutionEvent::Finished(_) => {
                    finished_events = finished_events.saturating_add(1);
                }
            }
        }
        let mut deferred_rebuilds: HashSet<PathBuf> = HashSet::new();
        for (path, event) in events {
            let mut status =
                Err("Notebook pane for finished execution was not found".to_string());
            let mut missing_python_kernel = false;
            let mut missing_python_kernel_retry = None;
            let finished = matches!(
                event,
                neoism_ui::editor::notebook::NotebookExecutionEvent::Finished(_)
            );
            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    let context = item.context_mut();
                    let Some(notebook) = context.notebook.as_mut() else {
                        continue;
                    };
                    if notebook.path == path {
                        status = match event.clone() {
                            neoism_ui::editor::notebook::NotebookExecutionEvent::Output(chunk) => {
                                deferred_rebuilds.insert(path.clone());
                                notebook.apply_execution_chunk_without_rebuild(chunk)
                            }
                            neoism_ui::editor::notebook::NotebookExecutionEvent::DisplayUpdate(update) => {
                                match notebook.apply_display_update_without_rebuild(update) {
                                    Ok(replaced) => {
                                        if replaced > 0 {
                                            deferred_rebuilds.insert(path.clone());
                                        }
                                        Ok(())
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            neoism_ui::editor::notebook::NotebookExecutionEvent::Finished(result) => {
                                missing_python_kernel = result.outputs.iter().any(|output| {
                                    output
                                        .get("ename")
                                        .and_then(serde_json::Value::as_str)
                                        == Some("JupyterKernelUnavailable")
                                });
                                if missing_python_kernel {
                                    missing_python_kernel_retry =
                                        Some((path.clone(), result.cell_index));
                                }
                                let apply_status =
                                    notebook.apply_execution_result_without_rebuild(result);
                                notebook.rebuild_markdown();
                                match apply_status {
                                    Ok(()) => notebook.save().map_err(|err| err.to_string()),
                                    Err(err) => Err(err),
                                }
                            }
                        };
                    }
                }
            }
            self.sync_markdown_tab_modified(&path, !finished || status.is_err());
            if finished {
                if missing_python_kernel {
                    if crate::notebook_runtime::has_managed_python_kernel() {
                        if let Some((retry_path, retry_cell_index)) =
                            missing_python_kernel_retry.as_ref()
                        {
                            match self
                                .retry_notebook_cell_after_python_kernel_install(
                                    retry_path,
                                    *retry_cell_index,
                                ) {
                                Ok(()) => {
                                    dirty = true;
                                    continue;
                                }
                                Err(err) => self.renderer.notifications.push(
                                    format!("Python kernel is installed, but retry failed: {err}"),
                                    neoism_ui::panels::notifications::NotificationLevel::Error,
                                ),
                            }
                        }
                        self.renderer.notifications.push(
                            "Python kernel is installed, but execution failed to start"
                                .to_string(),
                            neoism_ui::panels::notifications::NotificationLevel::Error,
                        );
                    } else {
                        self.pending_python_kernel_retry = missing_python_kernel_retry;
                        self.renderer.modal.open(neoism_ui::widgets::modal::ModalSpec {
                            title: "Python Kernel Required".to_string(),
                            body: "This notebook needs a Python Jupyter kernel. Neoism can install a managed Python kernel into its app data directory.".to_string(),
                            meta: "After install, Neoism will retry the failed cell.".to_string(),
                            input: None,
                            buttons: vec![
                                neoism_ui::widgets::modal::ModalButton::new(
                                    "Install Python Kernel",
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
                            blocking: false,
                        });
                    }
                } else {
                    match status {
                        Ok(()) => self.renderer.notifications.push(
                            "Notebook cell finished".to_string(),
                            neoism_ui::panels::notifications::NotificationLevel::Info,
                        ),
                        Err(err) => self.renderer.notifications.push(
                            format!("Notebook cell failed: {err}"),
                            neoism_ui::panels::notifications::NotificationLevel::Error,
                        ),
                    }
                }
            }
            dirty = true;
        }
        if !deferred_rebuilds.is_empty() {
            for grid in self.context_manager.contexts_mut() {
                for item in grid.contexts_mut().values_mut() {
                    let context = item.context_mut();
                    let Some(notebook) = context.notebook.as_mut() else {
                        continue;
                    };
                    if deferred_rebuilds.contains(&notebook.path) {
                        notebook.rebuild_markdown();
                    }
                }
            }
        }
        if budget_exhausted {
            dirty = true;
        }
        if event_count > 0 || budget_exhausted || disconnected_receivers > 0 {
            crate::app::freeze_watchdog::note_sampled(
                "notebook_poll",
                std::time::Duration::from_millis(500),
                format!(
                    "notebook_poll events={} outputs={} display_updates={} finished={} pending_receivers={} budget_exhausted={} disconnected_receivers={} elapsed_ms={} dirty={}",
                    event_count,
                    output_events,
                    display_events,
                    finished_events,
                    self.pending_notebook_executions.len(),
                    budget_exhausted,
                    disconnected_receivers,
                    poll_started.elapsed().as_millis(),
                    dirty
                ),
            );
        }
        if dirty {
            self.renderer.trail_cursor.reset();
            self.mark_dirty();
        }
        dirty
    }
}
