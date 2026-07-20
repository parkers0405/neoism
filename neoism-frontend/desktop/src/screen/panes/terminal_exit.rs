use super::*;

impl Screen<'_> {
    pub(crate) fn close_workspace_terminal_tab(&mut self, route_id: usize) {
        let workspace_id = self.current_workspace_id();
        tracing::info!(
            target: "neoism::editor_tabs",
            ?workspace_id,
            route_id,
            "closing workspace terminal buffer tab"
        );

        // Guest safety: a pane adopted from another host detaches
        // instead of `ClosePty`-ing the host's live shell.
        self.context_manager
            .detach_session_for_route_if_adopted(route_id);
        let _ = self
            .context_manager
            .should_close_context_manager(route_id, &mut self.sugarloaf);
        self.renderer.buffer_tabs.remove_terminal_route(route_id);

        if !self.renderer.buffer_tabs.tabs().is_empty() {
            let active = self.renderer.buffer_tabs.active();
            if !self.activate_workspace_buffer_tab(active) {
                self.reapply_chrome_layout();
                self.mark_dirty();
            }
        } else {
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }

    pub fn close_active_buffer_tab(&mut self) {
        self.close_active_buffer_tab_inner(false);
    }

    pub(crate) fn close_active_buffer_tab_inner(&mut self, return_to_terminal: bool) {
        let workspace_id = self.current_workspace_id();
        let current_is_editor = self.context_manager.current().code.is_some();
        if !self.renderer.buffer_tabs.is_visible() {
            tracing::warn!(
                target: "neoism::editor_tabs",
                ?workspace_id,
                current_is_editor,
                return_to_terminal,
                "close buffer ignored: buffer tabs are hidden"
            );
            return;
        }
        let active_ix = self.renderer.buffer_tabs.active();
        let remembered_path = workspace_id
            .as_ref()
            .and_then(|id| self.workspace_editor_active_paths.get(id))
            .cloned();
        tracing::info!(
            target: "neoism::editor_tabs",
            ?workspace_id,
            active_ix,
            active_is_terminal = self.renderer.buffer_tabs.is_terminal_at(active_ix),
            current_is_editor,
            return_to_terminal,
            remembered_path = remembered_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            tabs_len = self.renderer.buffer_tabs.tabs().len(),
            "close buffer requested"
        );
        let close_plan = self
            .renderer
            .buffer_tabs
            .active_close_plan(current_is_editor, remembered_path.as_deref());
        let active_ix = match close_plan {
            neoism_ui::panels::buffer_tabs::BufferTabClosePlan::CloseTerminalRoute {
                route_id,
            } => {
                self.close_workspace_terminal_tab(route_id);
                return;
            }
            neoism_ui::panels::buffer_tabs::BufferTabClosePlan::CloseTab { index } => {
                index
            }
            neoism_ui::panels::buffer_tabs::BufferTabClosePlan::Ignore => {
                tracing::warn!(
                    target: "neoism::editor_tabs",
                    ?workspace_id,
                    active_ix,
                    current_is_editor,
                    return_to_terminal,
                    "close buffer ignored: no file tab could be selected"
                );
                return;
            }
        };
        let (removed, new_active) = self.renderer.buffer_tabs.close_at(active_ix);
        let next_target = if return_to_terminal {
            None
        } else {
            new_active.clone()
        };
        let path_update =
            neoism_ui::panels::buffer_tabs::workspace_active_path_after_close(
                next_target.as_ref(),
                workspace_id.is_some(),
            );
        self.guard_workspace_buf_enter(path_update.buf_enter_guard());
        tracing::info!(
            target: "neoism::editor_tabs",
            ?workspace_id,
            active_ix,
            removed = neoism_ui::panels::buffer_tabs::buffer_tab_target_label(
                removed.as_ref()
            ),
            next_target = next_target
                .as_ref()
                .map(|target| neoism_ui::panels::buffer_tabs::buffer_tab_target_label(
                    Some(target)
                ))
                .unwrap_or_else(|| "<terminal>".to_string()),
            "closed Rust buffer tab"
        );
        self.apply_workspace_active_path_update(workspace_id.clone(), &path_update);
        if let Some(removed) = removed {
            match removed {
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                    self.notebook_runtime.shutdown_kernel(path.clone());
                    self.context_manager
                        .remove_markdown_by_path(&path, &mut self.sugarloaf);
                    self.context_manager
                        .remove_neoism_tags_by_path(&path, &mut self.sugarloaf);
                    tracing::info!(
                        target: "neoism::editor_tabs",
                        ?workspace_id,
                        path = %path.display(),
                        "closed markdown tab"
                    );
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(
                    route_id,
                ) => {
                    let _ = self
                        .context_manager
                        .remove_neoism_agent_route(route_id, &mut self.sugarloaf);
                    tracing::info!(
                        target: "neoism::neoism_agent",
                        route_id,
                        "closed Neoism agent tab"
                    );
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(page) => {
                    let _ = self
                        .context_manager
                        .remove_chrome_page_route(page.route_id, &mut self.sugarloaf);
                    tracing::info!(
                        target: "neoism::chrome_page",
                        kind = page.kind.title(),
                        route_id = page.route_id,
                        "closed chrome helper page tab"
                    );
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => {
                    let _ = self
                        .context_manager
                        .remove_code_by_path(&path, &mut self.sugarloaf);
                    tracing::info!(
                        target: "neoism::editor_tabs",
                        ?workspace_id,
                        path = %path.display(),
                        "closed code tab"
                    );
                }
            }
        }
        if return_to_terminal {
            tracing::info!(
                target: "neoism::editor_tabs",
                ?workspace_id,
                "returning to workspace Terminal tab after close"
            );
            self.activate_workspace_terminal_tab();
        } else if let Some(next) = new_active {
            match next {
                neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(path) => {
                    self.activate_markdown_path(path);
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(
                    route_id,
                ) => {
                    let ix = self.renderer.buffer_tabs.active();
                    let _ = self.activate_workspace_neoism_agent_route(ix, route_id);
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(page) => {
                    use neoism_ui::panels::buffer_tabs::ChromePageKind;
                    match page.kind {
                        ChromePageKind::Extensions => {
                            self.activate_neoism_extensions_page();
                        }
                    }
                }
                neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path) => {
                    self.activate_code_path(path);
                }
            }
        } else if self.renderer.buffer_tabs.active_is_terminal() {
            self.activate_workspace_terminal_tab();
        }
        self.mark_dirty();
    }

    pub fn handle_terminal_exit(&mut self, route_id: usize) -> bool {
        // Walk all grids via the shared `ContextGridLike` trait so the
        // "which grid owns this route" decision stays a pure function
        // testable in `neoism-ui`. The adapter newtype below threads
        // the desktop fork's `ContextGrid` into the trait's two query
        // points (`owns_route` + `describe_closing_route`).
        let grid_adapters: Vec<ContextGridDescriptorAdapter<'_>> = self
            .context_manager
            .all_grids()
            .iter()
            .map(ContextGridDescriptorAdapter)
            .collect();
        let closing = neoism_ui::session_layout::find_closing_workspace_descriptor(
            &grid_adapters,
            route_id as u64,
        );
        let closing_workspace_index = closing.map(|slot| slot.grid_index);
        let closing_workspace_id =
            closing.and_then(|slot| slot.workspace_id.map(|id| id as usize));
        let closing_is_workspace_root =
            closing.is_some_and(|slot| slot.is_workspace_root);
        let closing_shell_pid = closing.map(|slot| slot.shell_pid).unwrap_or(0);
        let closing_is_terminal_context =
            closing.is_some_and(|slot| slot.is_terminal_context);

        if closing_is_terminal_context && shell_pid_is_alive(closing_shell_pid) {
            tracing::warn!(
                target: "neoism::terminal_exit",
                route_id,
                shell_pid = closing_shell_pid,
                is_workspace_root = closing_is_workspace_root,
                "ignored terminal close event while shell pid is still alive"
            );
            return false;
        }

        let should_close_window = self
            .context_manager
            .should_close_context_manager(route_id, &mut self.sugarloaf);

        if !should_close_window {
            if closing_is_workspace_root {
                if let Some(id) = closing_workspace_id
                    .and_then(|id| self.context_manager.workspace_tree_id_for_route(id))
                {
                    self.workspace_roots.remove(&id);
                    self.workspace_buffer_tabs.remove(&id);
                    self.workspace_buf_enter_targets.remove(&id);
                    self.workspace_editor_active_paths.remove(&id);
                    if let (Some(island), Some(index)) =
                        (self.renderer.island.as_mut(), closing_workspace_index)
                    {
                        island.remove_tab_state(index);
                    }
                    self.load_current_workspace_chrome();
                    // Evict AFTER the load — see close_tab: the swap
                    // stashes the dead workspace's tree during load.
                    self.workspace_file_trees.remove(&id);
                    self.reapply_chrome_layout();
                }
            } else if let Some(id) = closing_workspace_id
                .and_then(|id| self.context_manager.workspace_tree_id_for_route(id))
            {
                if self.current_workspace_id().as_ref() == Some(&id) {
                    self.renderer.buffer_tabs.remove_terminal_route(route_id);
                    if !self.renderer.buffer_tabs.tabs().is_empty() {
                        let active = self.renderer.buffer_tabs.active();
                        let _ = self.activate_workspace_buffer_tab(active);
                    } else {
                        self.reapply_chrome_layout();
                    }
                } else if let Some(tabs) = self.workspace_buffer_tabs.get_mut(&id) {
                    tabs.remove_terminal_route(route_id);
                }
            }
            self.renderer.editor_scroll.reset_all();
            self.renderer.terminal_scroll.reset_all();
            self.renderer.trail_cursor.reset();
            self.mark_dirty();
        }

        should_close_window
    }
}
