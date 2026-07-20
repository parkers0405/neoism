// Extracted verbatim from screen/render/mod.rs render() pipeline.
// Phase A/B/C: workspace/watcher sync + status-line & chrome-strip
// drains + search-hint refresh. Pure code-move.
use super::*;

impl Screen<'_> {
    pub(crate) fn sync_status_and_chrome(&mut self, _ctx: &FrameCtx) {
        if self.sync_workspace_root_from_active_pane() {
            self.mark_dirty();
        }

        self.drain_workspace_note_index_events();

        self.sync_file_tree_watchers();

        let is_search_active = self.search_active();
        if is_search_active {
            if let Some(history_index) = self.search_state.history_index {
                self.renderer.set_active_search(
                    self.search_state.history.get(history_index).cloned(),
                );
            }
        } else {
            self.renderer.set_active_search(None);
        }

        // Refresh the chrome status line with whatever the active
        // pane represents — document file path or shell title — plus a
        // mode chip. Cheap (small allocs each frame); per-frame is fine
        // since the strip is always visible.
        {
            // Keep the LSP popup in sync while open. The nvim feed is
            // gone; the native editor will re-feed per-server rows
            // later. For now this keeps the aggregate rows fresh.
            if self.renderer.lsp_popup.is_visible() {
                self.populate_lsp_popup_for_current_buffer();
            }

            // Workspace-move feedback: flip the Workspaces modal's
            // "moving…" row to ✓/✗ when the async promote/demote POST
            // finishes, and keep redrawing while the spinner animates
            // or a result is on screen.
            if let Some(outcome) = self.context_manager.take_workspace_move_outcome() {
                let message = if outcome.ok {
                    String::new()
                } else {
                    outcome.message
                };
                self.renderer
                    .command_palette
                    .finish_workspace_move(outcome.ok, message);
            }
            if self.renderer.command_palette.tick_workspace_move() {
                self.mark_dirty();
            }

            let current = self.context_manager.current();
            let document_chrome_active = current.code.is_some()
                || current.markdown.is_some()
                || current.notebook.is_some();
            let (status_mode, primary, primary_kind, branch, active_path, active_cwd) =
                if let Some(code) = current.code.as_ref() {
                    let active_path = Some(code.path.clone());
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| code.path.parent().map(Path::to_path_buf));
                    let primary = code
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "(no file)".to_string());
                    let branch = active_path
                        .as_deref()
                        .or(cwd.as_deref())
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    // Vim panes report their modal state; standard
                    // input is always insert-like.
                    let mode = match (code.input_mode, code.buffer.mode) {
                        (
                            neoism_ui::editor::code::CodeInputMode::Vim,
                            neoism_ui::editor::code::CodeMode::Normal,
                        ) => neoism_ui::panels::status_line::Mode::Normal,
                        (
                            neoism_ui::editor::code::CodeInputMode::Vim,
                            neoism_ui::editor::code::CodeMode::Visual,
                        ) => neoism_ui::panels::status_line::Mode::Visual,
                        _ => neoism_ui::panels::status_line::Mode::Insert,
                    };
                    (
                        mode,
                        primary,
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        active_path,
                        cwd,
                    )
                } else if current.neoism_agent.is_some() {
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| std::env::current_dir().ok());
                    let branch = cwd
                        .as_deref()
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Agent,
                        "Neoism Agent".to_string(),
                        neoism_ui::panels::status_line::PrimaryKind::Agent,
                        branch,
                        None,
                        cwd,
                    )
                } else if let Some(tags) = current.neoism_tags.as_ref() {
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| Some(tags.workspace_root().to_path_buf()));
                    let branch = cwd
                        .as_deref()
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Markdown,
                        "Tags".to_string(),
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        Some(tags.path().to_path_buf()),
                        cwd,
                    )
                } else if let Some(markdown) = current.markdown.as_ref() {
                    let active_path = markdown.path.clone();
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| active_path.parent().map(Path::to_path_buf));
                    let primary = active_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Markdown".to_string());
                    let branch = Some(active_path.as_path())
                        .or(cwd.as_deref())
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Markdown,
                        primary,
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        Some(active_path),
                        cwd,
                    )
                } else if let Some(notebook) = current.notebook.as_ref() {
                    let active_path = notebook.path.clone();
                    let cwd = self
                        .active_workspace_root
                        .clone()
                        .or_else(|| active_path.parent().map(Path::to_path_buf));
                    let primary = active_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Notebook".to_string());
                    let branch = Some(active_path.as_path())
                        .or(cwd.as_deref())
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        neoism_ui::panels::status_line::Mode::Markdown,
                        primary,
                        neoism_ui::panels::status_line::PrimaryKind::File,
                        branch,
                        Some(active_path),
                        cwd,
                    )
                } else {
                    let terminal_agent =
                        self.renderer.buffer_tabs.agent_for_route(current.route_id);
                    let cwd_buf = current
                        .terminal
                        .lock()
                        .current_directory
                        .clone()
                        .or_else(|| self.active_workspace_root.clone())
                        .or_else(|| {
                            self.context_manager
                                .config
                                .working_dir
                                .clone()
                                .map(PathBuf::from)
                        })
                        .or_else(|| std::env::current_dir().ok());
                    let primary = terminal_agent
                        .map(|agent| agent.display_name().to_string())
                        .or_else(|| {
                            cwd_buf
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .map(|n| n.to_string_lossy().into_owned())
                        })
                        .unwrap_or_default();
                    let branch = cwd_buf
                        .as_deref()
                        .and_then(neoism_ui::panels::git_branch::branch_for);
                    (
                        if terminal_agent.is_some() {
                            neoism_ui::panels::status_line::Mode::Agent
                        } else {
                            neoism_ui::panels::status_line::Mode::Terminal
                        },
                        primary,
                        if terminal_agent.is_some() {
                            neoism_ui::panels::status_line::PrimaryKind::Agent
                        } else {
                            neoism_ui::panels::status_line::PrimaryKind::Terminal
                        },
                        branch,
                        None,
                        cwd_buf,
                    )
                };

            // Cwd pill (left side, between mode and branch) — populated
            // for every active-pane kind so the dir is always visible
            // and tracks whichever page the user is on. Document panes
            // use the active file's parent dir; terminal panes use the
            // shell's live cwd (which moves with `cd`). Path is
            // rendered zsh-style: `$HOME` collapses to `~`, and any
            // tail is kept (`~/projects/neoism`); paths outside `$HOME`
            // show verbatim.
            let cwd_label = active_path
                .as_deref()
                .and_then(|p| p.parent())
                .or(active_cwd.as_deref())
                .map(|p| {
                    if let Some(home) = std::env::var_os("HOME") {
                        let home = std::path::Path::new(&home);
                        if p == home {
                            return "~".to_string();
                        }
                        if let Ok(rel) = p.strip_prefix(home) {
                            return format!("~/{}", rel.display());
                        }
                    }
                    p.to_string_lossy().into_owned()
                });

            // Project pill (right side) — workspace root basename.
            let project = self
                .active_workspace_root
                .as_ref()
                .or(active_cwd.as_ref())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .or_else(|| {
                    self.context_manager
                        .config
                        .working_dir
                        .as_ref()
                        .and_then(|s| {
                            std::path::Path::new(s)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                        })
                });
            // Ruler pill: cursor line / total lines. Code and markdown
            // panes read the shared pane's cursor directly. Terminals
            // get no pill — a shell has no line position.
            let cursor_lines = if let Some(code) =
                self.context_manager.current().code.as_ref()
            {
                let total = code.buffer.line_count();
                (total > 0).then(|| ((code.buffer.cursor_line + 1).min(total), total))
            } else if let Some(markdown) =
                self.context_manager.current().markdown.as_ref()
            {
                let total = markdown.lines.len();
                (total > 0).then(|| ((markdown.cursor_line + 1).min(total), total))
            } else {
                None
            };
            let workspace = cursor_lines.map(|(cur, total)| format!("WS {cur}/{total}"));
            let git_changes = active_path
                .as_deref()
                .or(active_cwd.as_deref())
                .and_then(neoism_ui::panels::git_branch::change_summary_for)
                .filter(|summary| !summary.is_empty())
                .map(|summary| {
                    // `git_branch::GitChangeSummary` and
                    // `status_line::GitChangeSummary` are the same shape
                    // but distinct types after the cutover — the shared
                    // status_line crate has its own POD copy so it
                    // doesn't depend on `git_branch`'s filesystem code.
                    neoism_ui::panels::status_line::GitChangeSummary {
                        added: summary.added,
                        deleted: summary.deleted,
                    }
                });
            // nvim removed; native editor LSP/diagnostic feeds TBD.
            // LSP pill: connected server(s) for the active code file,
            // from the code bridge's worker-maintained cache.
            let (lsp_status, lsp_label) = self
                .context_manager
                .current()
                .code
                .as_ref()
                .and_then(|code| self.code_lsp_pill(&code.path))
                .map(|(status, label)| (Some(status), label))
                .unwrap_or((None, None));
            // Error/warning pills from the code LSP bridge's
            // diagnostics store (markdown/terminal panes show none).
            let diagnostics_counts = self
                .context_manager
                .current()
                .code
                .as_ref()
                .map(|code| self.code_diagnostic_counts(&code.path))
                .unwrap_or_default();
            self.renderer
                .buffer_tabs
                .set_visible(!self.renderer.buffer_tabs.tabs().is_empty());
            // Showcmd: in-progress vim keys on the mode chip
            // ("NORMAL · 2d"), code panes in vim mode only.
            let pending_keys = self
                .context_manager
                .current()
                .code
                .as_ref()
                .filter(|code| {
                    code.input_mode == neoism_ui::editor::code::CodeInputMode::Vim
                })
                .map(|code| code.buffer.vim.pending.showcmd())
                .filter(|pending| !pending.is_empty());
            self.renderer.status_line.set_info(
                neoism_ui::panels::status_line::StatusInfo {
                    mode: status_mode,
                    primary,
                    primary_kind,
                    branch,
                    git_changes,
                    workspace,
                    lsp_status,
                    lsp_label,
                    project,
                    cursor_lines,
                    pending_keys,
                    diagnostics: diagnostics_counts,
                    cwd_label,
                    fps: self
                        .renderer
                        .status_fps_enabled
                        .then(|| self.renderer.fps_counter.value())
                        .flatten(),
                },
            );

            // Breadcrumbs follow the active document tab. The buffer
            // tabs are the source of truth, so prefer the active Rust
            // tab when present. Always hide when the active pane is a
            // terminal — chrome tied to editor state shouldn't sit
            // above shell prompts.
            if document_chrome_active {
                let active_tab = self
                    .renderer
                    .buffer_tabs
                    .tabs()
                    .get(self.renderer.buffer_tabs.active());
                let crumb_path = match active_tab.and_then(|tab| tab.target()) {
                    Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::File(path)) => {
                        Some(path)
                    }
                    Some(neoism_ui::panels::buffer_tabs::BufferTabTarget::Markdown(
                        path,
                    )) => Some(path),
                    Some(
                        neoism_ui::panels::buffer_tabs::BufferTabTarget::NeoismAgent(_),
                    ) => {
                        // Agent pane has its own bottom status surface;
                        // suppress the global breadcrumb strip so a stale
                        // file path from a prior tab doesn't paint a black
                        // bar at the top.
                        self.renderer.breadcrumbs.set_segments(Vec::new());
                        self.renderer.breadcrumbs.clear_tail();
                        None
                    }
                    Some(
                        neoism_ui::panels::buffer_tabs::BufferTabTarget::ChromePage(_),
                    ) => {
                        // Chrome helper pages (Extensions, etc.) paint
                        // their own header so the global breadcrumb is
                        // suppressed — same treatment as the agent.
                        self.renderer.breadcrumbs.set_segments(Vec::new());
                        self.renderer.breadcrumbs.clear_tail();
                        None
                    }
                    None => active_path.clone(),
                };
                if let Some(notebook) = current.notebook.as_ref() {
                    use neoism_ui::panels::breadcrumbs::{
                        BreadcrumbAction, BreadcrumbActionItem,
                    };
                    self.renderer.breadcrumbs.set_actions(vec![
                        BreadcrumbActionItem::new(
                            "Run All",
                            BreadcrumbAction::RunAllNotebookCells,
                        ),
                        BreadcrumbActionItem::new(
                            "Clear All",
                            BreadcrumbAction::ClearNotebookOutputs,
                        ),
                        BreadcrumbActionItem::new(
                            "Interrupt",
                            BreadcrumbAction::InterruptNotebookKernel,
                        ),
                        BreadcrumbActionItem::new(
                            "Restart",
                            BreadcrumbAction::RestartNotebookKernel,
                        ),
                    ]);
                    self.renderer.breadcrumbs.set_notebook_kernel(
                        Some(notebook.kernel_display_label()),
                        self.renderer.context_menu.is_notebook_kernel(),
                    );
                    self.renderer.breadcrumbs.clear_tail();
                    self.renderer
                        .file_tree
                        .set_active_path(crumb_path.clone().or(active_path.clone()));
                } else if let Some(p) = crumb_path.clone() {
                    self.renderer
                        .breadcrumbs
                        .set_from_path(&p, active_cwd.as_deref());
                    // Code panes append the tree-sitter symbol trail
                    // (`mod net › impl Client › fn connect`).
                    match current.code.as_ref().filter(|c| !c.symbol_trail.is_empty())
                    {
                        Some(code) => {
                            let symbol = code
                                .symbol_trail
                                .iter()
                                .map(|s| s.name.as_str())
                                .collect::<Vec<_>>()
                                .join(" › ");
                            self.renderer.breadcrumbs.set_tail(
                                code.buffer.cursor_line as u64 + 1,
                                code.buffer.cursor_col as u64 + 1,
                                &symbol,
                            );
                        }
                        None => self.renderer.breadcrumbs.clear_tail(),
                    }
                    // Mirror the active buffer to the file-tree accent so
                    // the tree shows which file the editor is on, even
                    // when the user navigated there via finder/Tab and
                    // never clicked the tree row.
                    self.renderer.file_tree.set_active_path(crumb_path);
                } else if active_tab.is_none() {
                    self.renderer.breadcrumbs.set_segments(Vec::new());
                    self.renderer.breadcrumbs.clear_tail();
                    self.renderer.file_tree.set_active_path(None);
                } else {
                    self.renderer.breadcrumbs.clear_tail();
                    self.renderer.file_tree.set_active_path(None);
                }
            } else {
                self.renderer.breadcrumbs.set_segments(Vec::new());
                self.renderer.breadcrumbs.clear_tail();
                // Terminal panes have no "active file" — clear the
                // accent so the tree doesn't keep highlighting a row
                // that has nothing to do with the visible pane.
                self.renderer.file_tree.set_active_path(None);
            }

            // Keep the saved per-workspace Rust chrome snapshot fresh.
            // Route switches, tab activations, and terminal/agent tab
            // mutations all land here before a later workspace reload can
            // restore stale tab state.
            self.sync_current_workspace_chrome_snapshot();
        }

        // Enforce the chrome/grid geometry invariant before this frame
        // paints. This replaces the accidental "Ctrl+/- fixes line 1"
        // recovery with the same full reflow at the actual state
        // transition.
        self.repair_chrome_layout_if_stale();

        if is_search_active {
            // Update search hints in renderable content
            let mut search_terminal_busy = false;
            let hints =
                match { self.context_manager.current().terminal.try_lock_unfair() } {
                    Some(terminal) => self
                        .search_state
                        .dfas_mut()
                        .map(|dfas| HintMatches::visible_regex_matches(&terminal, dfas)),
                    None => {
                        search_terminal_busy = true;
                        None
                    }
                };
            if search_terminal_busy {
                self.context_manager
                    .current_mut()
                    .renderable_content
                    .pending_update
                    .set_dirty();
            }

            self.context_manager
                .current_mut()
                .renderable_content
                .hint_matches = hints.map(|h| h.iter().cloned().collect());

            // Force invalidation for search with full damage
            {
                let current = self.context_manager.current_mut();
                current
                    .renderable_content
                    .pending_update
                    .set_terminal_damage(
                        neoism_terminal_core::damage::TerminalDamage::Full,
                    );
            }
        }
    }
}
