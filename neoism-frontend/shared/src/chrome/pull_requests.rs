//! Host-neutral pull-request tab and reply ownership.
use super::*;
use neoism_protocol::pull_request::{
    PullRequestDetail, PullRequestErrorKind, PullRequestReply, PullRequestRequest,
};

impl<A: Send + Copy + 'static> Chrome<A> {
    fn focus_pull_request_page(&mut self) {
        self.git_diff_panel.set_focused(false);
        self.notes_sidebar.set_focused(false);
        self.buffer_tabs.set_focused(false);
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_focused(false);
        }
    }

    pub fn open_git_pull_request_inbox(
        &mut self,
        route: usize,
        root: std::path::PathBuf,
    ) {
        self.git_diff_panel.close();
        self.focus_pull_request_page();
        if !self
            .git_diff_views
            .get(&route)
            .is_some_and(|view| view.root == root && view.is_pull_request_inbox())
        {
            self.git_diff_views.insert(
                route,
                crate::panels::git_diff_view::GitDiffView::pull_request_inbox(root),
            );
            self.git_diff_view_generations.remove(&route);
            self.drain_review_header_actions(route);
        }
    }

    pub fn set_git_pull_request_view(
        &mut self,
        route: usize,
        root: std::path::PathBuf,
        detail: PullRequestDetail,
    ) {
        self.focus_pull_request_page();
        if let Some(view) = self.git_diff_views.get_mut(&route).filter(|view| {
            view.root == root && view.pull_request_number() == Some(detail.summary.number)
        }) {
            view.apply_pull_request_reply(&PullRequestReply::Detail { detail });
        } else {
            self.git_diff_views.insert(
                route,
                crate::panels::git_diff_view::GitDiffView::from_pull_request(
                    root, detail,
                ),
            );
        }
        self.git_diff_view_generations.remove(&route);
    }

    pub fn create_git_pull_request_view(
        &mut self,
        route: usize,
        root: std::path::PathBuf,
        branch: String,
        head: String,
        default_branch: String,
    ) {
        self.focus_pull_request_page();
        if self
            .git_diff_views
            .get(&route)
            .is_some_and(|view| view.root == root && view.is_pull_request())
        {
            return;
        }
        self.git_diff_views.insert(
            route,
            crate::panels::git_diff_view::GitDiffView::create_pull_request(
                root,
                branch,
                head,
                default_branch,
            ),
        );
        self.git_diff_view_generations.remove(&route);
    }

    pub fn git_pull_request_tab_info(&self, route: usize) -> Option<(u64, String)> {
        let view = self.git_diff_views.get(&route)?;
        Some((
            view.pull_request_number()?,
            view.pull_request_title()
                .unwrap_or("Pull request")
                .to_owned(),
        ))
    }

    pub fn git_pull_request_has_drafts(&self, route: usize) -> bool {
        self.git_diff_views
            .get(&route)
            .is_some_and(|view| view.has_pull_request_drafts())
    }

    pub fn apply_git_pull_request_reply(&mut self, reply: PullRequestReply) {
        self.git_diff_panel.host_pull_request_reply(reply);
        self.pump_git_pull_request_replies();
    }

    pub fn pump_git_pull_request_replies(&mut self) {
        for message in self.git_diff_panel.take_pull_request_notices() {
            self.notifications.push(
                message,
                crate::panels::notifications::NotificationLevel::Warn,
            );
        }
        for reply in self.git_diff_panel.take_pull_request_replies() {
            if let Some((route, root)) = self.pending_pull_request_route.take() {
                if self.git_diff_panel.repo_root().as_ref() == Some(&root) {
                    if let Some(view) = self
                        .git_diff_views
                        .get_mut(&route)
                        .filter(|view| view.root == root && view.is_pull_request())
                    {
                        view.apply_pull_request_reply(&reply);
                    }
                    self.drain_review_header_actions(route);
                }
            }
            if let PullRequestReply::CheckedOut {
                path,
                worktree: true,
                ..
            } = reply
            {
                self.pending_pull_request_workspaces.push(path);
            }
        }
    }

    pub(crate) fn dispatch_git_pull_request(
        &mut self,
        route: usize,
        root: std::path::PathBuf,
        request: PullRequestRequest,
    ) {
        if self.git_diff_panel.repo_root().as_ref() != Some(&root) {
            if let Some(view) = self.git_diff_views.get_mut(&route) {
                view.apply_pull_request_reply(&PullRequestReply::Error {
                    kind: PullRequestErrorKind::Unavailable,
                    message: "The connected repository changed. Reopen Pull Requests from its Git menu.".into(),
                });
            }
            return;
        }
        if matches!(
            request,
            PullRequestRequest::Checkout {
                new_worktree: false,
                ..
            }
        ) && (self.code_pane.as_ref().is_some_and(|pane| pane.is_dirty())
            || self.parked_code_panes.values().any(|pane| pane.is_dirty())
            || self
                .markdown_pane
                .as_ref()
                .is_some_and(|pane| pane.is_dirty()))
        {
            if let Some(view) = self.git_diff_views.get_mut(&route) {
                view.apply_pull_request_reply(&PullRequestReply::Error {
                    kind: PullRequestErrorKind::Conflict,
                    message:
                        "Save all open documents before checking out a pull request."
                            .into(),
                });
            }
            return;
        }
        if self.git_diff_panel.request_pull_request(request) {
            self.pending_pull_request_route = Some((route, root));
        } else if let Some(view) = self.git_diff_views.get_mut(&route) {
            view.apply_pull_request_reply(&PullRequestReply::Error { kind: PullRequestErrorKind::Conflict, message: "Another pull request operation is still running. Retry when it finishes.".into() });
        }
    }

    pub fn drain_pull_request_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_pull_request_urls)
    }

    pub fn drain_pull_request_workspaces(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_pull_request_workspaces)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reopening_inbox_keeps_its_state_and_is_not_a_creation_page() {
        let mut chrome = Chrome::<()>::new();
        chrome.git_diff_panel.open(Some("/repo".into()), None);
        chrome.open_git_pull_request_inbox(7, "/repo".into());
        assert!(!chrome.git_diff_panel.is_visible());
        assert!(chrome.git_diff_views[&7].is_pull_request_inbox());
        assert!(chrome.git_diff_views[&7].is_read_only());
        assert_eq!(chrome.git_pull_request_tab_info(7), None);
        chrome.open_git_pull_request_inbox(7, "/repo".into());
        assert!(chrome.git_diff_views[&7].is_pull_request_inbox());
        assert!(chrome.code_pane.is_none());
        assert!(chrome.markdown_pane.is_none());
    }

    #[test]
    fn creation_page_is_immutable_and_replay_preserves_its_draft() {
        let mut chrome = Chrome::<()>::new();
        let root = std::path::PathBuf::from("/repo");
        chrome.create_git_pull_request_view(
            7,
            root.clone(),
            "feature".into(),
            "a".repeat(40),
            "main".into(),
        );
        let view = chrome.git_diff_views.get_mut(&7).unwrap();
        assert!(view.is_read_only());
        assert!(view.pull_request_text_input("Keep this title"));
        assert!(view.has_pull_request_drafts());
        assert!(view.take_edit_request().is_none());
        chrome.create_git_pull_request_view(
            7,
            root,
            "feature".into(),
            "b".repeat(40),
            "main".into(),
        );
        assert!(chrome.git_pull_request_has_drafts(7));
        assert!(chrome.code_pane.is_none());
        assert!(chrome.markdown_pane.is_none());
    }

    #[test]
    fn printable_key_events_edit_pr_fields_without_entering_source_buffers() {
        use crate::event::{
            KeyDescriptor, KeyState, LogicalKey, Modifiers, PhysicalKey, UiEvent,
        };
        let mut chrome = Chrome::<()>::new();
        chrome
            .buffer_tabs
            .open_git_diff(7, std::path::Path::new(""));
        chrome.create_git_pull_request_view(
            7,
            "/repo".into(),
            "feature".into(),
            "a".repeat(40),
            "main".into(),
        );
        let key = UiEvent::Key(KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Character("A".into()),
            state: KeyState::Pressed,
            modifiers: Modifiers::SHIFT,
            repeat: false,
        });
        assert!(chrome.handle_git_diff_page_event(&key));
        assert!(chrome.handle_git_diff_page_event(&UiEvent::Text("é".into())));
        assert_eq!(chrome.git_diff_views[&7].pull_request_title(), Some("Aé"));
        assert!(chrome.code_pane.is_none());
    }

    #[test]
    fn reply_ownership_does_not_follow_the_selected_tab() {
        let mut chrome = Chrome::<()>::new();
        let root = std::path::PathBuf::from("/repo");
        chrome
            .git_diff_panel
            .open(Some(root.clone()), Some("feature".into()));
        chrome.create_git_pull_request_view(
            7,
            root.clone(),
            "feature".into(),
            "a".repeat(40),
            "main".into(),
        );
        chrome.create_git_pull_request_view(
            8,
            root.clone(),
            "other".into(),
            "b".repeat(40),
            "main".into(),
        );
        chrome
            .git_diff_views
            .get_mut(&7)
            .unwrap()
            .pull_request_text_input("Keep my draft");
        chrome.pending_pull_request_route = Some((7, root));
        chrome.apply_git_pull_request_reply(PullRequestReply::Error {
            kind: PullRequestErrorKind::Unavailable,
            message: "Offline".into(),
        });
        assert!(chrome.git_pull_request_has_drafts(7));
        assert!(!chrome.git_pull_request_has_drafts(8));
        assert!(chrome.pending_pull_request_route.is_none());
    }
}
