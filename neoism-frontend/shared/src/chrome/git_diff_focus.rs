//! Extend the existing chrome focus chain through the two diff columns.
use super::*;

impl<A: Send + Copy + 'static> Chrome<A> {
    pub(crate) fn focus_git_diff_column(&mut self, column: usize) -> bool {
        let Some(route) = self.active_git_diff_route() else {
            return false;
        };
        let Some(view) = self.git_diff_views.get_mut(&route) else {
            return false;
        };
        if !view.focus_column(column) {
            return false;
        }
        self.git_diff_panel.set_focused(false);
        self.buffer_tabs.set_focused(false);
        self.blur(PanelKey::BufferTabs);
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_focused(false);
        }
        self.blur(PanelKey::FileTree);
        self.notes_sidebar.set_focused(false);
        if let Some(pane) = self.agent_pane.as_mut() {
            pane.side_panel_mut().set_focused(false);
            pane.detail_panel_mut().set_focused(false);
        }
        true
    }

    pub(crate) fn handle_git_diff_file_navigation(&mut self, next: bool) -> bool {
        if self.git_diff_panel.is_focused()
            || self.buffer_tabs.is_focused()
            || self.chrome_overlay_active()
            || self.command_palette.is_enabled()
            || self.finder.is_enabled()
            || self.context_menu.is_visible()
            || self.share_sheet.is_visible()
            || self.conversation_navigator.is_active()
            || self.file_browser.is_active()
            || self.git_diff_panel.actions_menu_is_open()
            || self.git_diff_panel.branch_menu_is_open()
            || self
                .file_tree
                .as_ref()
                .is_some_and(|tree| tree.is_focused())
            || self.notes_sidebar.is_focused()
            || self.agent_pane.as_ref().is_some_and(|pane| {
                pane.side_panel().is_focused() || pane.detail_panel().is_focused()
            })
        {
            return false;
        }
        let Some(route) = self.active_git_diff_route() else {
            return false;
        };
        let Some(view) = self.git_diff_views.get_mut(&route) else {
            return false;
        };
        view.navigate_file(next);
        true
    }

    pub(crate) fn handle_git_diff_horizontal_focus(&mut self, right: bool) -> bool {
        if self.buffer_tabs.is_focused()
            || self.chrome_overlay_active()
            || self.git_diff_panel.actions_menu_is_open()
            || self.git_diff_panel.branch_menu_is_open()
        {
            return false;
        }
        let Some(route) = self.active_git_diff_route() else {
            return false;
        };
        if self.git_diff_panel.is_focused() {
            if right {
                return self.git_diff_panel.section_move_right();
            }
            if self.git_diff_panel.section_move_left() {
                return true;
            }
            return self.focus_git_diff_column(1);
        }
        if right
            && self.conversations_visible
            && self
                .agent_pane
                .as_ref()
                .is_some_and(|pane| pane.side_panel().is_focused())
            && !self
                .file_tree
                .as_ref()
                .is_some_and(|tree| tree.is_focused())
            && !self.notes_sidebar.is_focused()
        {
            return self.focus_git_diff_column(0);
        }
        if self
            .file_tree
            .as_ref()
            .is_some_and(|tree| tree.is_focused())
            || self.notes_sidebar.is_focused()
            || self.agent_pane.as_ref().is_some_and(|pane| {
                pane.side_panel().is_focused() || pane.detail_panel().is_focused()
            })
        {
            return false;
        }
        if self
            .git_diff_views
            .get_mut(&route)
            .is_some_and(|view| view.move_column_focus(right))
        {
            return true;
        }
        if right && self.git_diff_panel.is_visible() {
            if let Some(view) = self.git_diff_views.get_mut(&route) {
                view.blur_columns();
            }
            self.git_diff_panel.set_focused(true);
            return true;
        }
        if !right
            && (self.notes_sidebar.is_visible()
                || self
                    .file_tree
                    .as_ref()
                    .is_some_and(|tree| tree.is_visible())
                || self.conversations_visible)
        {
            if let Some(view) = self.git_diff_views.get_mut(&route) {
                view.blur_columns();
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{
        KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PhysicalKey, UiEvent,
    };
    use crate::panels::git_diff_view::GitDiffView;
    use crate::widgets::diff_card::{DiffLine, DiffLineKind};

    fn chrome() -> Chrome<()> {
        let mut chrome = Chrome::new();
        let index = chrome
            .buffer_tabs
            .open_git_diff(7, std::path::Path::new("a.rs"));
        chrome.active_tab_index = index;
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_file_lines(
            std::path::Path::new("a.rs"),
            vec![DiffLine {
                text: "new".into(),
                kind: DiffLineKind::Add,
                line_number: Some(1),
                old_line_number: None,
                new_line_number: Some(1),
            }],
        );
        chrome.git_diff_views.insert(7, view);
        chrome.toggle_git_diff_panel();
        chrome.git_diff_panel.set_focused(false);
        chrome
    }

    fn horizontal(chrome: &mut Chrome<()>, right: bool) {
        let key = KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Named(if right {
                NamedKey::ArrowRight
            } else {
                NamedKey::ArrowLeft
            }),
            modifiers: Modifiers::ALT,
            state: KeyState::Pressed,
            repeat: false,
        };
        assert!(chrome.handle_chrome_key_shortcut(&UiEvent::Key(key)));
    }

    fn vertical(chrome: &mut Chrome<()>, next: bool, modifiers: Modifiers) -> bool {
        chrome.handle_chrome_key_shortcut(&UiEvent::Key(KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Named(if next {
                NamedKey::ArrowDown
            } else {
                NamedKey::ArrowUp
            }),
            modifiers,
            state: KeyState::Pressed,
            repeat: false,
        }))
    }

    fn multi_file_chrome() -> Chrome<()> {
        use crate::panels::git_diff::{FileChange, FileStatus};
        let mut chrome = chrome();
        chrome.git_diff_views.get_mut(&7).unwrap().set_files(
            ["a.rs", "b.rs", "c.rs"]
                .into_iter()
                .map(|path| FileChange {
                    path: path.into(),
                    status: FileStatus::Modified,
                    additions: 1,
                    deletions: 0,
                    staged: false,
                })
                .collect(),
        );
        chrome
    }

    #[test]
    fn alt_vertical_arrows_visit_files_from_either_column_without_focusing_tabs() {
        for column in [0, 1] {
            let mut chrome = multi_file_chrome();
            assert!(chrome.focus_git_diff_column(column));
            assert!(vertical(&mut chrome, true, Modifiers::ALT));
            assert_eq!(chrome.git_diff_views[&7].path, std::path::Path::new("b.rs"));
            assert_eq!(chrome.git_diff_views[&7].column_focused(), Some(column));
            assert!(!chrome.buffer_tabs.is_focused());
            assert!(!chrome.git_diff_panel.is_focused());
            assert!(vertical(&mut chrome, true, Modifiers::ALT));
            assert_eq!(chrome.git_diff_views[&7].path, std::path::Path::new("c.rs"));
            assert!(vertical(&mut chrome, false, Modifiers::ALT));
            assert_eq!(chrome.git_diff_views[&7].path, std::path::Path::new("b.rs"));
        }
    }

    #[test]
    fn file_navigation_leaves_other_focus_owners_and_inactive_reviews_alone() {
        let mut chrome = multi_file_chrome();
        chrome.git_diff_panel.set_focused(true);
        assert!(!chrome.handle_git_diff_file_navigation(true));
        chrome.git_diff_panel.set_focused(false);
        chrome.buffer_tabs.set_focused(true);
        assert!(!chrome.handle_git_diff_file_navigation(true));
        chrome.buffer_tabs.set_focused(false);
        chrome.notes_sidebar.set_focused(true);
        assert!(!chrome.handle_git_diff_file_navigation(true));
        chrome.notes_sidebar.set_focused(false);
        chrome.command_palette.set_enabled(true);
        assert!(!chrome.handle_git_diff_file_navigation(true));
        chrome.command_palette.set_enabled(false);
        chrome.finder.set_enabled(true);
        assert!(!chrome.handle_git_diff_file_navigation(true));
        chrome.finder.set_enabled(false);
        chrome.active_tab_index = chrome.buffer_tabs.open_terminal(8);
        assert!(!chrome.handle_git_diff_file_navigation(true));
        assert_eq!(chrome.git_diff_views[&7].path, std::path::Path::new("a.rs"));
    }

    #[test]
    fn file_shortcut_requires_alt_only_and_does_not_repeat_on_release() {
        let mut chrome = multi_file_chrome();
        for modifiers in [
            Modifiers::empty(),
            Modifiers::ALT | Modifiers::SHIFT,
            Modifiers::ALT | Modifiers::CTRL,
            Modifiers::ALT | Modifiers::META,
        ] {
            vertical(&mut chrome, true, modifiers);
            assert_eq!(chrome.git_diff_views[&7].path, std::path::Path::new("a.rs"));
        }
        let released = UiEvent::Key(KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Named(NamedKey::ArrowDown),
            modifiers: Modifiers::ALT,
            state: KeyState::Released,
            repeat: false,
        });
        chrome.handle_chrome_key_shortcut(&released);
        assert_eq!(chrome.git_diff_views[&7].path, std::path::Path::new("a.rs"));
    }

    #[test]
    fn alt_arrows_visit_both_diff_columns_then_the_git_sidebar() {
        let mut chrome = chrome();
        horizontal(&mut chrome, false);
        assert_eq!(chrome.git_diff_views[&7].column_focused(), Some(0));
        horizontal(&mut chrome, true);
        assert_eq!(chrome.git_diff_views[&7].column_focused(), Some(1));
        assert!(chrome.git_diff_views[&7].editing_path().is_some());
        horizontal(&mut chrome, true);
        assert!(chrome.git_diff_panel.is_focused());
        assert!(chrome.git_diff_views[&7].column_focused().is_none());
        horizontal(&mut chrome, false);
        assert!(!chrome.git_diff_panel.is_focused());
        assert_eq!(chrome.git_diff_views[&7].column_focused(), Some(1));
        horizontal(&mut chrome, false);
        assert_eq!(chrome.git_diff_views[&7].column_focused(), Some(0));
    }

    #[test]
    fn sidebar_checkbox_is_a_focus_stop_before_returning_to_the_working_tree() {
        let mut chrome = chrome();
        chrome.git_diff_panel.set_focused(true);
        horizontal(&mut chrome, true);
        assert!(chrome.git_diff_panel.checkbox_column_focused());
        horizontal(&mut chrome, false);
        assert!(chrome.git_diff_panel.is_focused());
        assert!(!chrome.git_diff_panel.checkbox_column_focused());
        horizontal(&mut chrome, false);
        assert_eq!(chrome.git_diff_views[&7].column_focused(), Some(1));
    }
}
