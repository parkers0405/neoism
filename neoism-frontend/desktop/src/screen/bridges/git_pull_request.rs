//! Native hosting for immutable pull-request pages.
use super::super::*;
use neoism_protocol::pull_request::{
    PullRequestErrorKind, PullRequestReply, PullRequestRequest,
};
use neoism_ui::panels::git_diff::PullRequestOpen;
use neoism_ui::panels::git_diff_view::GitDiffView;

#[cfg(test)]
#[path = "git_pull_request_tests.rs"]
mod tests;

pub(crate) enum PullRequestConfirmation {
    Checkout {
        root: PathBuf,
        request: PullRequestRequest,
        route: usize,
        identity: String,
    },
    Navigate {
        path: PathBuf,
        identity: String,
    },
    CloseDraft {
        route: usize,
        identity: String,
    },
}

fn route_pull_request_actions(
    owner: usize,
    actions: Vec<neoism_ui::panels::git_diff_view::GitDiffViewAction>,
) -> Vec<(usize, neoism_ui::panels::git_diff_view::GitDiffViewAction)> {
    actions.into_iter().map(|action| (owner, action)).collect()
}

const CONFIRM_PULL_REQUEST: &str = "neoism.native.confirm-pull-request";

impl Screen<'_> {
    fn pull_request_host_identity(&mut self) -> String {
        let connection = self
            .context_manager
            .daemon_link_handle_and_runtime()
            .map(|(handle, _)| handle.connection_key());
        let workspace = self.context_manager.current_workspace_tree_id();
        let declared_root = self.context_manager.daemon_host_workspace_root(&workspace);
        format!(
            "{:?}",
            (
                connection,
                workspace,
                declared_root,
                self.context_manager.daemon_endpoint(),
                &self.active_workspace_root,
                self.context_manager.current_adopted_workspace_id(),
            )
        )
    }

    fn open_pull_request_confirmation(&mut self, title: &str, body: &str) {
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};
        self.renderer.modal.open(ModalSpec {
            title: title.into(),
            body: body.into(),
            meta: "No automatic stash or draft discard is performed.".into(),
            input: None,
            buttons: vec![
                ModalButton::new(
                    "Continue",
                    "Enter",
                    ModalAction::RunEditorCommand {
                        command: CONFIRM_PULL_REQUEST.into(),
                    },
                ),
                ModalButton::new("Cancel", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
    }

    pub(crate) fn confirm_pull_request_action(&mut self, command: &str) -> bool {
        if command != CONFIRM_PULL_REQUEST {
            return false;
        }
        self.renderer.modal.close();
        let Some(confirmation) = self.host_git.pull_request_confirmation.take() else {
            return true;
        };
        let identity = match &confirmation {
            PullRequestConfirmation::Checkout { identity, .. }
            | PullRequestConfirmation::Navigate { identity, .. }
            | PullRequestConfirmation::CloseDraft { identity, .. } => identity,
        };
        if *identity != self.pull_request_host_identity() {
            self.file_tree_notify(
                "The workspace changed. Retry the action from its original pull request.",
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            return true;
        }
        match confirmation {
            PullRequestConfirmation::Checkout {
                root,
                request,
                route,
                ..
            } => self.send_pull_request_for_route(root, request, route, true),
            PullRequestConfirmation::Navigate { path, .. } => {
                if self.has_dirty_code_owners()
                    || self.context_manager.all_grids().iter().any(|grid| {
                        grid.contexts().values().any(|item| {
                            item.context()
                                .markdown
                                .as_ref()
                                .is_some_and(|pane| pane.is_dirty())
                        })
                    })
                {
                    self.file_tree_notify("Save all open documents before opening the pull request worktree.", neoism_ui::panels::notifications::NotificationLevel::Warn);
                    return true;
                }
                if self.uses_host_git()
                    && !self.context_manager.current_workspace_is_remote_joined()
                {
                    self.file_tree_notify(format!("Worktree created on the repository host at {}. Open it through that host's workspace picker; it was not opened as a local path.", path.display()), neoism_ui::panels::notifications::NotificationLevel::Warn);
                    return true;
                }
                if let Err(error) = self.commit_workspace_directory(path) {
                    self.file_tree_notify(
                        error,
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    );
                }
            }
            PullRequestConfirmation::CloseDraft { route, .. } => {
                if let Some(index) =
                    self.renderer.buffer_tabs.tabs.iter().position(|tab| {
                        tab.chrome_page.is_some_and(|page| page.route_id == route)
                    })
                {
                    self.host_git.pull_request_close_approved = Some(route);
                    self.close_workspace_buffer_tab_at(index);
                    self.host_git.pull_request_close_approved = None;
                }
            }
        }
        true
    }

    pub(crate) fn confirm_pull_request_draft_close(&mut self, route: usize) {
        self.host_git.pull_request_confirmation =
            Some(PullRequestConfirmation::CloseDraft {
                route,
                identity: self.pull_request_host_identity(),
            });
        self.open_pull_request_confirmation("Discard pull request drafts?", "Closing this page will discard its unsent comments and form text. Submitted operations cannot be cancelled by closing the page.");
    }

    pub(crate) fn dispatch_pull_request(
        &mut self,
        root: PathBuf,
        request: PullRequestRequest,
    ) {
        let route = self.context_manager.current().route_id;
        self.dispatch_pull_request_for_route(root, request, route);
    }

    pub(crate) fn dispatch_pull_request_for_route(
        &mut self,
        root: PathBuf,
        request: PullRequestRequest,
        route: usize,
    ) {
        self.send_pull_request_for_route(root, request, route, false);
    }

    fn send_pull_request_for_route(
        &mut self,
        root: PathBuf,
        request: PullRequestRequest,
        route: usize,
        confirmed: bool,
    ) {
        self.sync_host_git();
        let identity = self.pull_request_host_identity();
        if self.host_git.pull_request_view_scopes.get(&route) != Some(&identity) {
            self.fail_pull_request_route(route, &root, "This page belongs to a previous repository host or workspace. Reopen the pull request from the Git sidebar.");
            return;
        }
        if self.renderer.git_diff_panel.repo_root().as_ref() != Some(&root) {
            self.fail_pull_request_route(
                route,
                &root,
                "The repository host changed. Reopen the pull request before retrying.",
            );
            return;
        }
        if matches!(request, PullRequestRequest::Checkout { .. })
            && self.context_manager.all_grids().iter().any(|grid| {
                grid.contexts().values().any(|item| {
                    item.context()
                        .code
                        .as_ref()
                        .is_some_and(|pane| pane.is_dirty())
                        || item
                            .context()
                            .markdown
                            .as_ref()
                            .is_some_and(|pane| pane.is_dirty())
                })
            })
        {
            self.fail_pull_request_route(
                route,
                &root,
                "Save all open documents before checking out a pull request.",
            );
            return;
        }
        if !confirmed
            && matches!(
                &request,
                PullRequestRequest::Checkout {
                    new_worktree: false,
                    ..
                }
            )
        {
            self.fail_pull_request_route(route, &root, "Checkout requires confirmation.");
            self.host_git.pull_request_confirmation =
                Some(PullRequestConfirmation::Checkout {
                    root,
                    request,
                    route,
                    identity: self.pull_request_host_identity(),
                });
            self.open_pull_request_confirmation("Change the current workspace branch?", "Checkout will change this workspace's branch and working files. Dirty working trees are refused by the repository host. Continue?");
            return;
        }
        if !self.dispatch_attached_pull_request(&root, request.clone(), route) {
            if self.renderer.git_diff_panel.request_pull_request(request) {
                self.set_pull_request_reply_route(route);
            } else {
                self.fail_pull_request_route(route, &root, "Another pull request operation is still running. Retry when it finishes.");
            }
        }
        self.mark_dirty();
    }

    /// Cancellation is the only reply allowed to cross an invalidated scope:
    /// it clears the original page's pending flag, never installs host data.
    pub(crate) fn fail_pull_request_route(
        &mut self,
        route: usize,
        root: &Path,
        message: &str,
    ) {
        let reply = PullRequestReply::Error {
            kind: PullRequestErrorKind::Unavailable,
            message: message.into(),
        };
        for grid in self.context_manager.all_grids_mut() {
            for item in grid.contexts_mut().values_mut() {
                if item.val.route_id == route {
                    if let Some(view) = item
                        .val
                        .git_diff_view
                        .as_mut()
                        .filter(|view| view.root == root && view.is_pull_request())
                    {
                        view.apply_pull_request_reply(&reply);
                    }
                }
            }
        }
        self.mark_dirty();
    }

    pub(crate) fn cancel_native_pull_request(&mut self, route: usize, root: &Path) {
        let message = "The repository host changed or reconnected. Verify the operation on GitHub before retrying; it will not be replayed.";
        self.fail_pull_request_route(route, root, message);
        self.set_pull_request_reply_route(route);
        self.renderer
            .git_diff_panel
            .host_pull_request_reply(PullRequestReply::Error {
                kind: PullRequestErrorKind::Unavailable,
                message: message.into(),
            });
    }

    pub(crate) fn pump_pull_request_views(&mut self) {
        for message in self.renderer.git_diff_panel.take_pull_request_notices() {
            self.file_tree_notify(
                message,
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
        }
        for reply in self.renderer.git_diff_panel.take_pull_request_replies() {
            let route = self.take_pull_request_reply_route();
            let root = self.renderer.git_diff_panel.repo_root();
            let mut actions = Vec::new();
            let mut titles = Vec::new();
            for grid in self.context_manager.all_grids_mut() {
                for item in grid.contexts_mut().values_mut() {
                    if let Some(view) = item.val.git_diff_view.as_mut() {
                        if root.as_ref() == Some(&view.root)
                            && view.is_pull_request()
                            && route.is_some_and(|route| route == item.val.route_id)
                        {
                            view.apply_pull_request_reply(&reply);
                            if let Some(number) = view.pull_request_number() {
                                titles.push((
                                    item.val.route_id,
                                    format!(
                                        "#{} {}",
                                        number,
                                        view.pull_request_title()
                                            .unwrap_or("Pull request")
                                    ),
                                ));
                            }
                            actions.extend(route_pull_request_actions(
                                item.val.route_id,
                                view.take_actions(),
                            ));
                        }
                    }
                }
            }
            for (route, title) in titles {
                for tabs in std::iter::once(&mut self.renderer.buffer_tabs)
                    .chain(self.renderer.pane_tabs.values_mut())
                    .chain(self.workspace_buffer_tabs.values_mut())
                {
                    if let Some(index) = tabs.tabs.iter().position(|tab| {
                        tab.chrome_page.is_some_and(|page| page.route_id == route)
                    }) {
                        tabs.set_title(index, &title);
                    }
                }
            }
            for (route, action) in actions {
                if let neoism_ui::panels::git_diff_view::GitDiffViewAction::PullRequest { root, request } = action {
                    self.dispatch_pull_request_for_route(root, request, route);
                }
            }
            match reply {
                PullRequestReply::CheckedOut {
                    path,
                    worktree: true,
                    ..
                } => {
                    // Never silently rehome the workspace after a background reply.
                    // Navigation is a second, explicit client-host action, scoped to
                    // the workspace which requested the worktree.
                    if route.is_some() {
                        self.host_git.pull_request_confirmation =
                            Some(PullRequestConfirmation::Navigate {
                                path: PathBuf::from(&path),
                                identity: self.pull_request_host_identity(),
                            });
                        self.open_pull_request_confirmation("Open pull request worktree?", &format!("The repository host created `{path}`. Open it as this workspace's declared directory?"));
                    }
                }
                PullRequestReply::CheckedOut { .. } => {
                    self.renderer.git_diff_panel.refresh(root.clone(), None)
                }
                _ => {}
            }
        }
        for open in self.renderer.git_diff_panel.take_pull_request_opens() {
            let mut snapshot_reply = None;
            let (root, number, title, view) = match open {
                PullRequestOpen::Inbox { root } => {
                    let view = GitDiffView::pull_request_inbox(root.clone());
                    (root, None, "Pull Requests".to_owned(), view)
                }
                PullRequestOpen::Create {
                    root,
                    branch,
                    head,
                    default_branch,
                } => {
                    let view = GitDiffView::create_pull_request(
                        root.clone(),
                        branch,
                        head,
                        default_branch,
                    );
                    (root, None, "New pull request".to_owned(), view)
                }
                PullRequestOpen::Detail { root, detail } => {
                    let number = detail.summary.number;
                    snapshot_reply = Some(PullRequestReply::Detail {
                        detail: detail.clone(),
                    });
                    let title = format!("#{} {}", number, detail.summary.title);
                    let view = GitDiffView::from_pull_request(root.clone(), detail);
                    (root, Some(number), title, view)
                }
            };
            let inbox = view.is_pull_request_inbox();
            let identity = self.pull_request_host_identity();
            let existing = self
                .context_manager
                .current_grid()
                .contexts()
                .values()
                .find_map(|item| {
                    item.val
                        .git_diff_view
                        .as_ref()
                        .filter(|view| {
                            view.root == root
                                && view.is_pull_request()
                                && view.is_pull_request_inbox() == inbox
                                && view.pull_request_number() == number
                                && self
                                    .host_git
                                    .pull_request_view_scopes
                                    .get(&item.val.route_id)
                                    == Some(&identity)
                        })
                        .map(|_| item.val.route_id)
                });
            let route = if let Some(route) = existing {
                if let Some(reply) = &snapshot_reply {
                    if let Some(item) = self
                        .context_manager
                        .current_grid_mut()
                        .contexts_mut()
                        .values_mut()
                        .find(|item| item.val.route_id == route)
                    {
                        if let Some(view) = item.val.git_diff_view.as_mut() {
                            view.apply_pull_request_reply(reply);
                        }
                    }
                }
                route
            } else {
                let rich_text = crate::context::factories::next_rich_text_id();
                let _ = self.sugarloaf.text(Some(rich_text));
                let Some(route) = self.context_manager.add_stacked_git_diff(
                    root,
                    PathBuf::new(),
                    rich_text,
                    &mut self.sugarloaf,
                ) else {
                    continue;
                };
                if let Some(item) = self
                    .context_manager
                    .current_grid_mut()
                    .contexts_mut()
                    .values_mut()
                    .find(|item| item.val.route_id == route)
                {
                    item.val.git_diff_view = Some(view);
                    item.val.git_diff_load_deadline = None;
                }
                route
            };
            self.host_git
                .pull_request_view_scopes
                .insert(route, identity);
            let mut initial_actions = Vec::new();
            for item in self
                .context_manager
                .current_grid_mut()
                .contexts_mut()
                .values_mut()
            {
                if item.val.route_id == route {
                    if let Some(view) = item.val.git_diff_view.as_mut() {
                        initial_actions = view.take_actions();
                    }
                    break;
                }
            }
            for action in initial_actions {
                if let neoism_ui::panels::git_diff_view::GitDiffViewAction::PullRequest { root, request } = action {
                    self.dispatch_pull_request_for_route(root, request, route);
                }
            }
            if inbox {
                self.renderer.git_diff_panel.close();
            }
            self.renderer.git_diff_panel.set_focused(false);
            self.renderer.file_tree.set_focused(false);
            self.renderer.notes_sidebar.set_focused(false);
            self.renderer.buffer_tabs.set_focused(false);
            self.renderer.buffer_tabs.ensure_terminal_tab();
            self.activate_git_diff_route(route);
            let index = self
                .renderer
                .buffer_tabs
                .open_git_diff(route, Path::new(""));
            self.renderer.buffer_tabs.set_title(index, &title);
            self.reapply_chrome_layout();
            self.mark_dirty();
        }
    }
}
