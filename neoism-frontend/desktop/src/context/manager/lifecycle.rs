use super::*;
use crate::context::factories::create_dead_context;
use crate::context::renderable::Cursor;
use crate::context::title::ContextManagerTitles;
use crate::event::RioEvent;
use crate::layout::{ContextDimension, ContextGrid};
use neoism_backend::config::layout::Margin;
use neoism_backend::error::{RioError, RioErrorLevel, RioErrorType};
use neoism_backend::event::EventListener;
use neoism_backend::event::WindowId;
use neoism_backend::sugarloaf::{font::SugarloafFont, Sugarloaf, SugarloafErrors};
use neoism_ui::session_layout::close_unfocused_tabs_plan;
use smallvec::smallvec;
use std::error::Error;
use std::time::Instant;

impl<T: EventListener + Clone + std::marker::Send + Sync + 'static> ContextManager<T> {
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        cursor_state: (&Cursor, bool),
        event_proxy: T,
        window_id: WindowId,
        route_id: usize,
        rich_text_id: usize,
        ctx_config: ContextManagerConfig,
        size: ContextDimension,
        scaled_margin: Margin,
        sugarloaf_errors: Option<SugarloafErrors>,
    ) -> Result<Self, Box<dyn Error>> {
        let initial_context = match ContextManager::create_context(
            cursor_state,
            event_proxy.clone(),
            window_id,
            rich_text_id,
            size,
            &ctx_config,
            // The daemon link attaches after construction; the first
            // pane is always local-PTY backed.
            None,
        ) {
            Ok(context) => context,
            Err(err_message) => {
                tracing::error!("{:?}", err_message);

                event_proxy.send_event(
                    RioEvent::ReportToAssistant(RioError {
                        report: RioErrorType::InitializationError(
                            err_message.to_string(),
                        ),
                        level: RioErrorLevel::Error,
                    }),
                    window_id,
                );

                create_dead_context(
                    event_proxy.clone(),
                    window_id,
                    route_id,
                    0,
                    ContextDimension::default(),
                )
            }
        };

        let titles = ContextManagerTitles::new(0, String::from("tab"), None);
        let initial_route_id = initial_context.route_id;
        tracing::trace!(
            target: "neoism::context",
            initial_route_id,
            "context manager initial route selected"
        );

        // Sugarloaf has found errors and context need to notify it for the user
        if let Some(errors) = sugarloaf_errors {
            if !errors.fonts_not_found.is_empty() {
                event_proxy.send_event(
                    RioEvent::ReportToAssistant({
                        RioError {
                            report: RioErrorType::FontsNotFound(errors.fonts_not_found),
                            level: RioErrorLevel::Warning,
                        }
                    }),
                    window_id,
                );
            }
        }

        Ok(ContextManager {
            current_index: 0,
            current_route: initial_route_id,
            contexts: smallvec![ContextGrid::new(
                initial_context,
                scaled_margin,
                ctx_config.split_color,
                ctx_config.split_active_color,
                ctx_config.panel,
            )],
            capacity: DEFAULT_CONTEXT_CAPACITY,
            event_proxy,
            window_id,
            config: ctx_config,
            titles,
            daemon: ContextManagerDaemonState::default(),
        })
    }

    #[cfg(test)]
    pub fn start_with_capacity(
        capacity: usize,
        event_proxy: T,
        window_id: WindowId,
    ) -> Result<Self, Box<dyn Error>> {
        let config = ContextManagerConfig {
            #[cfg(not(target_os = "windows"))]
            use_fork: true,
            working_dir: None,
            shell: Shell {
                program: std::env::var("SHELL").unwrap_or("bash".to_string()),
                args: vec![],
            },
            spawn_performer: false,
            is_native: false,
            should_update_title_extra: false,
            cwd: false,
            ..ContextManagerConfig::default()
        };
        let initial_context = ContextManager::create_context(
            (&Cursor::default(), false),
            event_proxy.clone(),
            window_id,
            0,
            ContextDimension::default(),
            &config,
            None,
        )?;

        let titles = ContextManagerTitles::new(0, String::new(), None);
        let initial_route_id = initial_context.route_id;

        Ok(ContextManager {
            current_index: 0,
            current_route: initial_route_id,
            contexts: smallvec![ContextGrid::new(
                initial_context,
                Margin::default(),
                config.split_color,
                config.split_active_color,
                config.panel,
            )],
            capacity,
            event_proxy,
            window_id,
            config,
            titles,
            daemon: ContextManagerDaemonState::default(),
        })
    }

    #[inline]
    pub fn should_close_context_manager(
        &mut self,
        route_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        // should_close_context_manager is only called when terminal.exit()
        // is triggered. The terminal.exit() happens for any drop on context
        // by tab removal or if the Pty is exited (e.g: exit/control+d)
        //
        // In the tab case we already have removed the context with the
        // specified route_id so isn't gonna find anything. Then will be false.
        //
        // However if the tab is killed by Pty and not a tab action then
        // it means we need to clean the context with the specified route_id.
        // If there's no context then should return true and kill the window.
        //
        // The branch decision (drop grid vs drop just the route vs
        // already-untracked) is the renderer-neutral
        // `neoism_ui::context_policy::route_exit_plan`. This method only
        // does the mutation that the plan recommends.
        let host = self.contexts.iter().enumerate().find_map(|(index, grid)| {
            grid.node_by_route_id(route_id).map(|node| {
                (
                    index,
                    node,
                    grid.workspace_route_id() == Some(route_id),
                    grid.len(),
                )
            })
        });

        let plan_input = neoism_ui::context_policy::RouteExitInput {
            contexts_len: self.contexts.len(),
            host_grid_index: host.map(|(idx, _, _, _)| idx),
            is_workspace_root: host.map(|(_, _, is_root, _)| is_root).unwrap_or(false),
            host_grid_len: host.map(|(_, _, _, len)| len).unwrap_or(0),
        };
        match neoism_ui::context_policy::route_exit_plan(plan_input) {
            neoism_ui::context_policy::RouteExitPlan::Untracked { contexts_empty } => {
                contexts_empty
            }
            neoism_ui::context_policy::RouteExitPlan::RemoveGrid { grid_index } => {
                // The root terminal owns the top-level workspace. If it
                // exits, close that whole workspace even when a stacked
                // editor peer is currently visible over it.
                self.contexts[grid_index].remove_all_rich_text(sugarloaf);
                self.contexts.remove(grid_index);
                self.remove_title_at_index(grid_index);

                if self.contexts.is_empty() {
                    return true;
                }

                if self.current_index == grid_index {
                    self.current_index = grid_index.min(self.contexts.len() - 1);
                } else if self.current_index > grid_index {
                    self.current_index -= 1;
                }
                self.current_route = self.current().route_id;
                self.keep_only_active_context_visible(sugarloaf);
                false
            }
            neoism_ui::context_policy::RouteExitPlan::RemoveRoute { grid_index } => {
                let node = host
                    .map(|(_, node, _, _)| node)
                    .expect("RemoveRoute implies host grid was found");
                self.contexts[grid_index].remove_node(node, sugarloaf);
                if self.current_index == grid_index {
                    self.current_route = self.contexts[grid_index].current().route_id;
                }
                false
            }
        }
    }

    #[inline]
    pub fn request_render(&mut self) {
        self.event_proxy
            .send_event(RioEvent::RenderRoute(self.current_route), self.window_id);
    }

    #[inline]
    pub fn blink_cursor(&mut self, scheduled_time: u64) {
        // PrepareRender will force a render for any route that is focused on window
        // PrepareRenderOnRoute only call render function for specific route ids.
        self.event_proxy.send_event(
            RioEvent::BlinkCursor(scheduled_time, self.current_route),
            self.window_id,
        );
    }

    #[inline]
    pub fn report_error_fonts_not_found(&mut self, fonts_not_found: Vec<SugarloafFont>) {
        if !fonts_not_found.is_empty() {
            self.event_proxy.send_event(
                RioEvent::ReportToAssistant({
                    RioError {
                        report: RioErrorType::FontsNotFound(fonts_not_found),
                        level: RioErrorLevel::Warning,
                    }
                }),
                self.window_id,
            );
        }
    }

    #[inline]
    pub fn create_new_window(&self) {
        self.event_proxy
            .send_event(RioEvent::CreateWindow(None), self.window_id);
    }

    #[inline]
    pub fn close_unfocused_tabs(&mut self) {
        let Some(plan) =
            close_unfocused_tabs_plan(self.contexts.len(), self.current_index)
        else {
            return;
        };

        let retained_title = self.titles.titles.remove(&plan.retained_index);
        self.titles.titles.clear();
        if let Some(title) = retained_title {
            self.titles.titles.insert(plan.active_index_after, title);
        }

        for index in plan.remove_indices_desc {
            self.contexts.remove(index);
        }
        self.current_index = plan.active_index_after;
        self.current_route = self.current().route_id;
    }

    #[inline]
    pub fn set_last_typing(&mut self) {
        self.current_mut().renderable_content.last_typing = Some(Instant::now());
    }
}
