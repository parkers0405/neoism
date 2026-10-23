//! Lifecycle policy for canonical documents projected into a review page.
use crate::screen::Screen;
use neoism_ui::panels::buffer_tabs::{BufferTabTarget, ChromePageKind};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

fn register_review_dirty_paths<A>(
    tabs: &mut neoism_ui::panels::buffer_tabs::BufferTabs<A>,
    paths: &[PathBuf],
) {
    let active = tabs.active();
    for path in paths {
        tabs.open_path(path.clone());
        tabs.set_modified(path, true);
    }
    tabs.set_active(active);
}

fn set_review_page_flags<A>(
    tabs: &mut neoism_ui::panels::buffer_tabs::BufferTabs<A>,
    states: &[(usize, bool)],
) {
    for tab in &mut tabs.tabs {
        if let Some(page) = tab.chrome_page {
            if let Some((_, dirty)) =
                states.iter().find(|(route, _)| *route == page.route_id)
            {
                tab.modified = *dirty;
            }
        }
    }
}

impl Screen<'_> {
    pub(crate) fn has_pull_request_drafts(&self) -> bool {
        self.context_manager.all_grids().iter().any(|grid| grid.contexts().values().any(|item| {
            item.context().git_diff_view.as_ref().is_some_and(|view| view.has_pull_request_drafts())
        }))
    }

    pub(crate) fn has_dirty_code_owners(&self) -> bool {
        self.context_manager.all_grids().iter().any(|grid| {
            grid.contexts().values().any(|item| {
                item.context()
                    .code
                    .as_ref()
                    .is_some_and(|code| code.is_dirty())
            })
        })
    }

    pub(crate) fn current_grid_has_dirty_code_owners(&self) -> bool {
        self.context_manager
            .current_grid()
            .contexts()
            .values()
            .any(|item| {
                item.context()
                    .code
                    .as_ref()
                    .is_some_and(|code| code.is_dirty())
            })
    }

    /// Closing a projection does not close its documents. Register any dirty
    /// background owners before removing the page, preserving the current tab
    /// selection until the ordinary close policy selects a real remaining tab.
    pub(crate) fn prepare_review_tab_close(&mut self, index: usize) -> bool {
        let Some(BufferTabTarget::ChromePage(page)) =
            self.renderer.buffer_tabs.target_at(index)
        else {
            return true;
        };
        if page.kind != ChromePageKind::GitDiff {
            return true;
        }
        if self.host_git.pull_request_close_approved != Some(page.route_id)
            && self.context_manager.current_grid().contexts().values().any(|item| {
            item.context().route_id == page.route_id
                && item.context().git_diff_view.as_ref().is_some_and(|view| view.has_pull_request_drafts())
        }) {
            self.confirm_pull_request_draft_close(page.route_id);
            return false;
        }
        let Some(root) = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .find_map(|item| {
                let context = item.context();
                (context.route_id == page.route_id)
                    .then_some(context.git_diff_view.as_ref())
                    .flatten()
                    .filter(|view| !view.is_read_only())
                    .map(|view| view.root.clone())
            })
        else {
            return true;
        };
        let current = self.context_manager.current_index();
        let identity = self
            .context_manager
            .adopted_workspace_identity_for_route(page.route_id);
        let stranded_foreign = self
            .context_manager
            .all_grids()
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != current)
            .flat_map(|(_, grid)| grid.contexts().values())
            .any(|item| {
                let owner = item.context();
                self.context_manager
                    .adopted_workspace_identity_for_route(owner.route_id)
                    == identity
                    && owner.code.as_ref().is_some_and(|code| {
                        code.is_dirty()
                            && neoism_protocol::host_path::HostPath::new(
                                root.to_string_lossy(),
                            )
                            .relative(&code.path.to_string_lossy())
                            .is_some()
                            && !self.code_path_has_registered_tab(&code.path)
                    })
            });
        if stranded_foreign {
            self.file_tree_notify("Save the background source owner before closing its review; it belongs to another workspace", neoism_ui::panels::notifications::NotificationLevel::Warn);
            return false;
        }
        let paths: Vec<_> = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .filter_map(|item| item.context().code.as_ref())
            .filter(|code| code.is_dirty())
            .map(|code| code.path.clone())
            .collect();
        let missing: Vec<_> = paths
            .iter()
            .filter(|path| !self.code_path_has_registered_tab(path))
            .cloned()
            .collect();
        register_review_dirty_paths(&mut self.renderer.buffer_tabs, &missing);
        for path in paths {
            self.sync_markdown_tab_modified(&path, true);
        }
        true
    }

    fn code_path_has_registered_tab(&self, path: &Path) -> bool {
        self.renderer.buffer_tabs.tabs().iter().chain(self.renderer.pane_tabs.values().chain(self.workspace_buffer_tabs.values()).flat_map(|tabs| tabs.tabs().iter()))
            .any(|tab| matches!(tab.target(), Some(BufferTabTarget::File(owner)) if owner == path))
    }

    pub(crate) fn refuse_dirty_code_tab_close(&mut self, index: usize) -> bool {
        let Some(BufferTabTarget::File(path)) =
            self.renderer.buffer_tabs.target_at(index)
        else {
            return false;
        };
        if !self
            .context_manager
            .code_pane_mut_by_path(&path)
            .is_some_and(|code| code.is_dirty())
        {
            return false;
        }
        self.file_tree_notify(
            "Save the canonical source document before closing its tab",
            neoism_ui::panels::notifications::NotificationLevel::Warn,
        );
        true
    }

    /// A terminal fallback must never label a cached root CodePane 'Terminal'.
    /// Register that owner only when actually falling back to it, not on the
    /// initial review click, and keep its text/history/CRDT binding unchanged.
    pub(crate) fn promote_code_fallback(&mut self, path: PathBuf) {
        let dirty = self
            .context_manager
            .code_pane_mut_by_path(&path)
            .is_some_and(|code| code.is_dirty());
        self.renderer.buffer_tabs.open_path(path.clone());
        self.renderer.buffer_tabs.set_modified(&path, dirty);
        self.activate_code_path(path);
    }

    /// Keep route-backed review tabs honest even after editing focus blurs.
    /// A repository review owns a multi-file presentation, not a second buffer.
    pub(crate) fn sync_review_document_modified(&mut self) {
        let states: Vec<_> = self
            .context_manager
            .all_grids()
            .iter()
            .flat_map(|grid| {
                grid.contexts().values().filter_map(|item| {
                    let context = item.context();
                    let view = context.git_diff_view.as_ref()?;
                    if view.is_read_only() {
                        return Some((context.route_id, view.has_pull_request_drafts()));
                    }
                    let root = neoism_protocol::host_path::HostPath::new(
                        view.root.to_string_lossy(),
                    );
                    let identity = self
                        .context_manager
                        .adopted_workspace_identity_for_route(context.route_id);
                    let dirty = self
                        .context_manager
                        .all_grids()
                        .iter()
                        .flat_map(|grid| grid.contexts().values())
                        .any(|item| {
                            let owner = item.context();
                            self.context_manager
                                .adopted_workspace_identity_for_route(owner.route_id)
                                == identity
                                && owner.code.as_ref().is_some_and(|code| {
                                    code.is_dirty()
                                        && root
                                            .relative(&code.path.to_string_lossy())
                                            .is_some()
                                })
                        });
                    Some((context.route_id, dirty))
                })
            })
            .collect();
        for tabs in std::iter::once(&mut self.renderer.buffer_tabs)
            .chain(self.renderer.pane_tabs.values_mut())
            .chain(self.workspace_buffer_tabs.values_mut())
        {
            set_review_page_flags(tabs, &states);
        }
    }
}
