use super::*;
use crate::context::factories::{
    create_code_context, create_draw_context, create_markdown_context,
    create_neoism_agent_context, create_neoism_extensions_context,
    create_neoism_tags_context, create_notebook_context, process_open_url,
};
use crate::event::RioEvent;
use crate::layout::ContextGrid;
use neoism_backend::event::EventListener;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_protocol::workspace::{PaneLayoutOp, PaneSplitAxis, PaneSplitPlacement};
use std::path::PathBuf;

impl<T: EventListener + Clone + std::marker::Send + Sync + 'static> ContextManager<T> {
    pub fn split(
        &mut self,
        rich_text_id: usize,
        split_down: bool,
        working_dir_override: Option<PathBuf>,
        sugarloaf: &mut Sugarloaf,
    ) {
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::Split {
                axis: if split_down {
                    PaneSplitAxis::Vertical
                } else {
                    PaneSplitAxis::Horizontal
                },
                placement: PaneSplitPlacement::After,
            },
        ) {
            return;
        }
        let mut working_dir = working_dir_override
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| self.config.working_dir.clone());
        if working_dir.is_none()
            && self.config.cwd
            && self.current().markdown.is_none()
            && self.current().neoism_agent.is_none()
        {
            #[cfg(not(target_os = "windows"))]
            {
                let current_context = self.current();
                if let Ok(path) = neoism_terminal_pty::foreground_process_path(
                    *current_context.main_fd,
                    current_context.shell_pid,
                ) {
                    working_dir = Some(path.to_string_lossy().to_string());
                }
            }

            #[cfg(target_os = "windows")]
            {
                // if let Ok(path) = neoism_terminal_pty::foreground_process_path() {
                //     working_dir =
                //         Some(path.to_string_lossy().to_string());
                // }
                working_dir = None;
            }
        }

        let mut cloned_config = self.config.clone();
        if working_dir.is_some() {
            cloned_config.working_dir = working_dir;
        }

        let current = self.current();
        let cursor = current.cursor_from_ref();

        match ContextManager::create_context(
            (&cursor, self.config.cursor_blinking),
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            self.current().dimension,
            &cloned_config,
            self.prepared_remote_pty(),
        ) {
            Ok(new_context) => {
                self.register_remote_context(&new_context);
                let new_route_id = new_context.route_id;
                if split_down {
                    self.contexts[self.current_index].split_down(new_context, sugarloaf);
                } else {
                    self.contexts[self.current_index].split_right(new_context, sugarloaf);
                }

                self.current_route = new_route_id;
            }
            Err(..) => {
                tracing::error!("not able to create a new context");
            }
        }
    }

    pub fn split_existing_route(
        &mut self,
        route_id: usize,
        split_down: bool,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        if self.request_pane_layout_op(
            route_id as u64,
            PaneLayoutOp::Split {
                axis: if split_down {
                    PaneSplitAxis::Vertical
                } else {
                    PaneSplitAxis::Horizontal
                },
                placement: PaneSplitPlacement::After,
            },
        ) {
            return true;
        }
        let Some(node) = self.contexts[self.current_index].node_by_route_id(route_id)
        else {
            return false;
        };

        let moved = if split_down {
            self.contexts[self.current_index].split_existing_down(node, sugarloaf)
        } else {
            self.contexts[self.current_index].split_existing_right(node, sugarloaf)
        };
        if moved {
            self.current_route = route_id;
            self.sync_daemon_workspaces();
        }
        moved
    }

    pub fn stack_existing_route_on_workspace(
        &mut self,
        route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let Some(node) = self.contexts[self.current_index].node_by_route_id(route_id)
        else {
            return false;
        };
        let moved =
            self.contexts[self.current_index].stack_existing_on_root(node, sugarloaf);
        if moved {
            self.current_route = route_id;
            self.sync_daemon_workspaces();
        }
        moved
    }

    pub fn stack_existing_route_on_route(
        &mut self,
        route_id: usize,
        parent_route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let Some(node) = self.contexts[self.current_index].node_by_route_id(route_id)
        else {
            return false;
        };
        let Some(parent) =
            self.contexts[self.current_index].node_by_route_id(parent_route_id)
        else {
            return false;
        };
        let moved = self.contexts[self.current_index]
            .stack_existing_on_parent(node, parent, sugarloaf);
        if moved {
            self.current_route = route_id;
            self.sync_daemon_workspaces();
        }
        moved
    }

    pub fn split_from_config(
        &mut self,
        rich_text_id: usize,
        split_down: bool,
        config: neoism_backend::config::Config,
        sugarloaf: &mut Sugarloaf,
    ) {
        if self.request_pane_layout_op(
            self.current_route as u64,
            PaneLayoutOp::Split {
                axis: if split_down {
                    PaneSplitAxis::Vertical
                } else {
                    PaneSplitAxis::Horizontal
                },
                placement: PaneSplitPlacement::After,
            },
        ) {
            return;
        }

        let (shell, working_dir) = process_open_url(
            config.shell.to_owned(),
            config.working_dir.to_owned(),
            config.editor.to_owned(),
            None,
        );

        let context_manager_config = ContextManagerConfig {
            cwd: config.navigation.current_working_directory,
            shell,
            working_dir,
            spawn_performer: true,
            #[cfg(not(target_os = "windows"))]
            use_fork: config.use_fork,
            is_native: config.navigation.is_native(),
            // When navigation is collapsed and does not contain any color rule
            // does not make sense fetch for foreground process names
            should_update_title_extra: !config.navigation.color_automation.is_empty(),
            split_color: config.colors.split,
            split_active_color: config.colors.split_active,
            panel: config.panel,
            title: config.title,
            keyboard: config.keyboard,
            scrollback_history_limit: config.scrollback_history_limit,
            ide_theme: config.neoism.theme,
            cursor_blinking: config.cursor.blinking,
        };

        let current = self.current();
        let cursor = current.cursor_from_ref();

        match ContextManager::create_context(
            (&cursor, context_manager_config.cursor_blinking),
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            self.current().dimension,
            &context_manager_config,
            self.prepared_remote_pty(),
        ) {
            Ok(new_context) => {
                self.register_remote_context(&new_context);
                let new_route_id = new_context.route_id;
                if split_down {
                    self.contexts[self.current_index].split_down(new_context, sugarloaf);
                } else {
                    self.contexts[self.current_index].split_right(new_context, sugarloaf);
                }

                self.current_route = new_route_id;
                self.sync_daemon_workspaces();
            }
            Err(..) => {
                tracing::error!("not able to create a new context");
            }
        }
    }

    #[inline]
    #[allow(dead_code)]
    pub fn add_context(&mut self, redirect: bool, rich_text_id: usize) {
        self.add_context_with_working_dir(redirect, rich_text_id, None);
    }

    #[inline]
    pub fn add_context_with_working_dir(
        &mut self,
        redirect: bool,
        rich_text_id: usize,
        working_dir_override: Option<PathBuf>,
    ) {
        let mut working_dir = working_dir_override
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| self.config.working_dir.clone());
        if working_dir.is_none() && self.config.cwd {
            #[cfg(not(target_os = "windows"))]
            {
                let current_context = self.current();
                if let Ok(path) = neoism_terminal_pty::foreground_process_path(
                    *current_context.main_fd,
                    current_context.shell_pid,
                ) {
                    working_dir = Some(path.to_string_lossy().to_string());
                }
            }

            #[cfg(target_os = "windows")]
            {
                // if let Ok(path) = neoism_terminal_pty::foreground_process_path() {
                //     working_dir =
                //         Some(path.to_string_lossy().to_string());
                // }
                working_dir = None;
            }
        }

        if self.config.is_native {
            self.event_proxy
                .send_event(RioEvent::CreateNativeTab(working_dir), self.window_id);
            return;
        }

        self.request_new_session(working_dir.clone(), None);

        let size = self.contexts.len();
        if size < self.capacity {
            let last_index = self.contexts.len();

            let mut cloned_config = self.config.clone();
            if working_dir.is_some() {
                cloned_config.working_dir = working_dir;
            }

            let current = self.current();
            let cursor = current.cursor_from_ref();
            let mut dimension = current.dimension;

            // If current has splits then shouldn't use that dimension
            if self.current_grid().len() > 1 {
                dimension = self.current_grid().grid_dimension();
            }

            match ContextManager::create_context(
                (&cursor, self.config.cursor_blinking),
                self.event_proxy.clone(),
                self.window_id,
                rich_text_id,
                dimension,
                &cloned_config,
                self.prepared_remote_pty(),
            ) {
                Ok(new_context) => {
                    self.register_remote_context(&new_context);
                    let previous_scaled_margin =
                        self.contexts[self.current_index].scaled_margin;
                    self.contexts.push(ContextGrid::new(
                        new_context,
                        previous_scaled_margin,
                        self.config.split_color,
                        self.config.split_active_color,
                        self.config.panel,
                    ));
                    if redirect {
                        self.current_index = last_index;
                        self.current_route = self.current().route_id;
                    }
                    let workspace_id = desktop_workspace_id(
                        self.window_id,
                        &self.contexts[last_index],
                        last_index,
                    );
                    self.request_daemon_workspace_create(
                        workspace_id,
                        None,
                        self.config.working_dir.as_ref().map(PathBuf::from),
                    );
                    self.sync_daemon_workspaces();
                }
                Err(..) => {
                    tracing::error!("not able to create a new context");
                }
            }
        }
    }

    pub fn markdown_node_by_path(
        &self,
        path: &std::path::Path,
    ) -> Option<(usize, taffy::NodeId)> {
        self.contexts.get(self.current_index).and_then(|grid| {
            grid.contexts().iter().find_map(|(node, item)| {
                item.context()
                    .markdown
                    .as_ref()
                    .filter(|pane| pane.path.as_path() == path)
                    .map(|_| (item.context().route_id, *node))
            })
        })
    }

    /// Mutable markdown pane lookup by path across EVERY grid — a
    /// daemon file-content reply may land after the user has switched
    /// islands, so the current-grid-only search is not enough.
    pub fn markdown_pane_mut_by_path(
        &mut self,
        path: &std::path::Path,
    ) -> Option<&mut neoism_ui::editor::markdown::MarkdownPane> {
        self.contexts.iter_mut().find_map(|grid| {
            grid.contexts_mut().values_mut().find_map(|item| {
                item.context_mut()
                    .markdown
                    .as_mut()
                    .filter(|pane| pane.path.as_path() == path)
            })
        })
    }

    pub fn draw_node_by_path(
        &self,
        path: &std::path::Path,
    ) -> Option<(usize, taffy::NodeId)> {
        self.contexts.get(self.current_index).and_then(|grid| {
            grid.contexts().iter().find_map(|(node, item)| {
                item.context()
                    .draw
                    .as_ref()
                    .filter(|pane| pane.path.as_path() == path)
                    .map(|_| (item.context().route_id, *node))
            })
        })
    }

    pub fn neoism_tags_node_by_path(
        &self,
        path: &std::path::Path,
    ) -> Option<(usize, taffy::NodeId)> {
        self.contexts.get(self.current_index).and_then(|grid| {
            grid.contexts().iter().find_map(|(node, item)| {
                item.context()
                    .neoism_tags
                    .as_ref()
                    .filter(|pane| pane.path() == path)
                    .map(|_| (item.context().route_id, *node))
            })
        })
    }

    /// Locate the singleton Extensions context in the current grid, if any.
    /// Extensions is a page (not per-path), so no path filter applies.
    pub(crate) fn neoism_extensions_node(&self) -> Option<(usize, taffy::NodeId)> {
        self.contexts.get(self.current_index).and_then(|grid| {
            grid.contexts().iter().find_map(|(node, item)| {
                item.context()
                    .neoism_extensions
                    .as_ref()
                    .map(|_| (item.context().route_id, *node))
            })
        })
    }

    pub fn remove_markdown_by_path(
        &mut self,
        path: &std::path::Path,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let Some(node) = self.contexts[self.current_index]
            .contexts()
            .iter()
            .find_map(|(node, item)| {
                let context = item.context();
                context
                    .markdown
                    .as_ref()
                    .filter(|pane| pane.path.as_path() == path)
                    .map(|_| *node)
                    .or_else(|| {
                        context
                            .notebook
                            .as_ref()
                            .filter(|pane| pane.path.as_path() == path)
                            .map(|_| *node)
                    })
            })
        else {
            return false;
        };

        self.contexts[self.current_index].remove_node(node, sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        true
    }

    /// Close the native code pane showing `path` (mirrors
    /// `remove_markdown_by_path`).
    pub fn remove_code_by_path(
        &mut self,
        path: &std::path::Path,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let Some(node) = self.contexts[self.current_index]
            .contexts()
            .iter()
            .find_map(|(node, item)| {
                item.context()
                    .code
                    .as_ref()
                    .filter(|pane| pane.path.as_path() == path)
                    .map(|_| *node)
            })
        else {
            return false;
        };

        self.contexts[self.current_index].remove_node(node, sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        true
    }

    pub fn remove_neoism_tags_by_path(
        &mut self,
        path: &std::path::Path,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let Some(node) = self.contexts[self.current_index]
            .contexts()
            .iter()
            .find_map(|(node, item)| {
                item.context()
                    .neoism_tags
                    .as_ref()
                    .filter(|pane| pane.path() == path)
                    .map(|_| *node)
            })
        else {
            return false;
        };

        self.contexts[self.current_index].remove_node(node, sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        true
    }

    pub fn can_remove_neoism_agent_route(&self, route_id: usize) -> bool {
        let grid = &self.contexts[self.current_index];
        let Some((node, item)) = grid
            .contexts()
            .iter()
            .find(|(_, item)| item.context().route_id == route_id)
        else {
            return false;
        };

        item.context().neoism_agent.is_some()
            && grid.len() > 1
            && grid.workspace_route_id() != Some(route_id)
            && grid.node_by_route_id(route_id) == Some(*node)
    }

    pub fn remove_neoism_agent_route(
        &mut self,
        route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        if !self.can_remove_neoism_agent_route(route_id) {
            return false;
        }

        let Some(node) = self.contexts[self.current_index].node_by_route_id(route_id)
        else {
            return false;
        };

        self.contexts[self.current_index].remove_node(node, sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        true
    }

    /// Close the singleton chrome helper-page context (Extensions, etc.).
    /// Buffer-tab strip routes close intents through this; the next
    /// open re-creates a fresh context. Safe to call when the context
    /// doesn't exist — no-ops.
    pub fn remove_chrome_page_route(
        &mut self,
        route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let Some(node) = self.contexts[self.current_index].node_by_route_id(route_id)
        else {
            return false;
        };
        self.contexts[self.current_index].remove_node(node, sugarloaf);
        self.current_route = self.contexts[self.current_index].current().route_id;
        true
    }

    pub fn code_node_by_path(
        &self,
        path: &std::path::Path,
    ) -> Option<(usize, taffy::NodeId)> {
        self.contexts.get(self.current_index).and_then(|grid| {
            grid.contexts().iter().find_map(|(node, item)| {
                item.context()
                    .code
                    .as_ref()
                    .filter(|pane| pane.path.as_path() == path)
                    .map(|_| (item.context().route_id, *node))
            })
        })
    }

    /// Mutable code pane lookup by path across every grid (mirrors
    /// `markdown_pane_mut_by_path`).
    pub fn code_pane_mut_by_path(
        &mut self,
        path: &std::path::Path,
    ) -> Option<&mut neoism_ui::editor::code::CodePane> {
        self.contexts.iter_mut().find_map(|grid| {
            grid.contexts_mut().values_mut().find_map(|item| {
                item.context_mut()
                    .code
                    .as_mut()
                    .filter(|pane| pane.path.as_path() == path)
            })
        })
    }

    pub fn add_stacked_code(
        &mut self,
        file: PathBuf,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_code_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
        );
        let new_route_id = new_context.route_id;
        if self.contexts[self.current_index]
            .add_stacked_context(new_context, sugarloaf)
            .is_some()
        {
            self.current_route = new_route_id;
            true
        } else {
            false
        }
    }

    pub fn add_stacked_markdown(
        &mut self,
        file: PathBuf,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_markdown_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
        );
        let new_route_id = new_context.route_id;
        if self.contexts[self.current_index]
            .add_stacked_context(new_context, sugarloaf)
            .is_some()
        {
            self.current_route = new_route_id;
            true
        } else {
            false
        }
    }

    pub fn add_stacked_draw(
        &mut self,
        file: PathBuf,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_draw_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
        );
        let new_route_id = new_context.route_id;
        if self.contexts[self.current_index]
            .add_stacked_context(new_context, sugarloaf)
            .is_some()
        {
            self.current_route = new_route_id;
            true
        } else {
            false
        }
    }

    pub fn add_stacked_notebook(
        &mut self,
        file: PathBuf,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_notebook_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
        );
        let new_route_id = new_context.route_id;
        if self.contexts[self.current_index]
            .add_stacked_context(new_context, sugarloaf)
            .is_some()
        {
            self.current_route = new_route_id;
            true
        } else {
            false
        }
    }

    pub fn add_stacked_neoism_tags(
        &mut self,
        file: PathBuf,
        workspace_root: PathBuf,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_neoism_tags_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
            workspace_root,
        );
        let new_route_id = new_context.route_id;
        if self.contexts[self.current_index]
            .add_stacked_context(new_context, sugarloaf)
            .is_some()
        {
            self.current_route = new_route_id;
            true
        } else {
            false
        }
    }

    pub fn add_stacked_neoism_extensions(
        &mut self,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_neoism_extensions_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
        );
        let new_route_id = new_context.route_id;
        if self.contexts[self.current_index]
            .add_stacked_context(new_context, sugarloaf)
            .is_some()
        {
            self.current_route = new_route_id;
            true
        } else {
            false
        }
    }

    pub fn add_stacked_neoism_agent(
        &mut self,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
        directory: Option<String>,
    ) -> Option<usize> {
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_neoism_agent_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            directory,
        );
        let new_route_id = new_context.route_id;
        self.contexts[self.current_index].add_stacked_context(new_context, sugarloaf)?;
        self.current_route = new_route_id;
        Some(new_route_id)
    }

    pub fn add_stacked_markdown_on_route(
        &mut self,
        file: PathBuf,
        parent_route_id: usize,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> Option<usize> {
        let parent =
            self.contexts[self.current_index].node_by_route_id(parent_route_id)?;
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_markdown_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
        );
        let new_route_id = new_context.route_id;
        self.contexts[self.current_index].add_stacked_context_on_parent(
            new_context,
            parent,
            sugarloaf,
        )?;
        self.current_route = new_route_id;
        Some(new_route_id)
    }

    /// Like [`Self::add_stacked_markdown_on_route`] but for the native
    /// code editor — stacks a new code pane on the pane hosting
    /// `parent_route_id`.
    pub fn add_stacked_code_on_route(
        &mut self,
        file: PathBuf,
        parent_route_id: usize,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> Option<usize> {
        let parent =
            self.contexts[self.current_index].node_by_route_id(parent_route_id)?;
        let dimension = self.current_grid().grid_dimension();
        let new_context = create_code_context(
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            file,
        );
        let new_route_id = new_context.route_id;
        self.contexts[self.current_index].add_stacked_context_on_parent(
            new_context,
            parent,
            sugarloaf,
        )?;
        self.current_route = new_route_id;
        Some(new_route_id)
    }

    /// Like [`Self::add_stacked_terminal`] but stacks the new terminal on
    /// the pane hosting `parent_route` (a secondary split pane) instead of
    /// the workspace root — backs the per-pane "+" new-terminal button.
    pub fn add_stacked_terminal_on_route(
        &mut self,
        parent_route: usize,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
        cwd: Option<PathBuf>,
    ) -> Option<usize> {
        let parent_node =
            self.contexts[self.current_index].node_by_route_id(parent_route)?;
        let mut cloned_config = self.config.clone();
        if let Some(cwd) = cwd.as_ref() {
            cloned_config.working_dir = Some(cwd.to_string_lossy().to_string());
        }
        self.request_new_session(cloned_config.working_dir.clone(), None);

        let cursor = self.current().cursor_from_ref();
        let dimension = self.current_grid().grid_dimension();
        match ContextManager::create_context(
            (&cursor, self.config.cursor_blinking),
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            &cloned_config,
            self.prepared_remote_pty(),
        ) {
            Ok(new_context) => {
                self.register_remote_context(&new_context);
                let route_id = new_context.route_id;
                self.contexts[self.current_index].add_stacked_context_on_parent(
                    new_context,
                    parent_node,
                    sugarloaf,
                )?;
                self.current_route = route_id;
                Some(route_id)
            }
            Err(e) => {
                tracing::error!("pane stacked terminal create failed: {e}");
                None
            }
        }
    }

    pub fn add_stacked_terminal(
        &mut self,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
        cwd: Option<PathBuf>,
    ) -> Option<usize> {
        let mut cloned_config = self.config.clone();
        if let Some(cwd) = cwd.as_ref() {
            cloned_config.working_dir = Some(cwd.to_string_lossy().to_string());
        }
        self.request_new_session(cloned_config.working_dir.clone(), None);

        let current = self.current();
        let cursor = current.cursor_from_ref();
        let dimension = self.current_grid().grid_dimension();

        match ContextManager::create_context(
            (&cursor, self.config.cursor_blinking),
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            &cloned_config,
            self.prepared_remote_pty(),
        ) {
            Ok(new_context) => {
                self.register_remote_context(&new_context);
                let route_id = new_context.route_id;
                self.contexts[self.current_index]
                    .add_stacked_context(new_context, sugarloaf)?;
                self.current_route = route_id;
                Some(route_id)
            }
            Err(e) => {
                tracing::error!("stacked terminal create failed: {e}");
                None
            }
        }
    }

    /// Hide all rich text components except for the current tab
    #[inline]
    pub fn keep_only_active_context_visible(&self, sugarloaf: &mut Sugarloaf) {
        for (idx, context) in self.contexts.iter().enumerate() {
            // Skip the current tab
            if idx == self.current_index {
                context.set_render_visibility(sugarloaf, true);
                continue;
            }

            context.set_render_visibility(sugarloaf, false);
        }
    }

    /// Switch visibility between two contexts (hide old, show new)
    #[inline]
    pub fn switch_context_visibility(
        &self,
        sugarloaf: &mut Sugarloaf,
        old_index: usize,
        new_index: usize,
    ) {
        if let Some(old_context) = self.contexts.get(old_index) {
            old_context.set_render_visibility(sugarloaf, false);
        }
        if let Some(new_context) = self.contexts.get(new_index) {
            new_context.set_render_visibility(sugarloaf, true);
        }
    }
}
