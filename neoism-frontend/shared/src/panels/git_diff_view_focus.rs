//! Spatial keyboard focus inside the before/working-tree review columns.
use super::*;

impl GitDiffView {
    pub fn set_host_focused(&mut self, focused: bool) {
        self.host_focused = focused;
    }

    pub fn column_focused(&self) -> Option<usize> {
        self.column_focus
    }

    pub fn blur_columns(&mut self) {
        self.column_focus = None;
        self.edit_focused = false;
        self.reveal_edit_on_focus = false;
    }

    /// Visit a file header without letting the previous source caret pull the
    /// viewport back. Its canonical document stays attached for live painting.
    pub fn navigate_file(&mut self, next: bool) {
        if self.sections.is_empty() {
            return;
        }
        let last = self.sections.len() - 1;
        let index = match self.indices.get(&self.path) {
            Some(&index) if next => (index + 1).min(last),
            Some(&index) => index.saturating_sub(1),
            None if next => 0,
            None => last,
        };
        let path = PathBuf::from(&self.sections[index].change.path);
        self.edit_focused = false;
        self.reveal_edit_on_focus = false;
        self.edit_request = None;
        self.reveal_file(&path);
    }

    pub fn move_column_focus(&mut self, right: bool) -> bool {
        let current = self.column_focus.unwrap_or(1);
        match (current, right, self.effective_split) {
            (0, true, _) => self.focus_column(1),
            (1, false, true) => self.focus_column(0),
            _ => false,
        }
    }

    pub fn focus_column(&mut self, column: usize) -> bool {
        if column > 1 {
            return false;
        }
        let column = if self.effective_split { column } else { 1 };
        self.column_focus = Some(column);
        self.active_column = column;
        self.edit_focused = false;
        self.reveal_edit_on_focus = false;
        if column == 0 || self.is_read_only() {
            return true;
        }
        if self.edit_path.as_ref() == Some(&self.path) {
            self.edit_focused = true;
            self.reveal_edit_on_focus = true;
            return true;
        }
        let Some(section) = self
            .indices
            .get(&self.path)
            .and_then(|index| self.sections.get(*index))
        else {
            return true;
        };
        let Some(line) = section
            .lines
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .filter_map(|line| line.new_line_number)
            .find(|line| *line > 0)
        else {
            return true;
        };
        self.edit_path = Some(self.path.clone());
        self.edit_request = Some(GitDiffEditRequest {
            root: self.root.clone(),
            path: self.path.clone(),
            line: line as usize - 1,
            column: 0,
        });
        self.edit_focused = true;
        self.reveal_edit_on_focus = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review() -> GitDiffView {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_file_lines(
            Path::new("a.rs"),
            vec![DiffLine {
                text: "working tree".into(),
                kind: DiffLineKind::Add,
                line_number: Some(42),
                old_line_number: None,
                new_line_number: Some(42),
            }],
        );
        view
    }

    fn multi_file_review() -> GitDiffView {
        let mut view = review();
        view.set_files(
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
        view
    }

    #[test]
    fn file_navigation_expands_headers_preserves_columns_and_clamps_at_edges() {
        for column in [0, 1] {
            let mut view = multi_file_review();
            view.focus_column(column);
            view.sections[1].collapsed = true;
            view.navigate_file(false);
            assert_eq!(view.path, Path::new("a.rs"));
            view.navigate_file(true);
            assert_eq!(view.path, Path::new("b.rs"));
            assert!(!view.sections[1].collapsed);
            assert_eq!(view.pending_reveal.as_deref(), Some(Path::new("b.rs")));
            assert_eq!(view.column_focused(), Some(column));
            view.navigate_file(true);
            view.navigate_file(true);
            assert_eq!(view.path, Path::new("c.rs"));
            assert!(view.key("["));
            assert_eq!(view.path, Path::new("b.rs"));
            assert!(view.key("]"));
            assert_eq!(view.path, Path::new("c.rs"));
        }
    }

    #[test]
    fn file_navigation_handles_empty_and_missing_selection() {
        let mut view = multi_file_review();
        view.path = "missing.rs".into();
        view.navigate_file(true);
        assert_eq!(view.path, Path::new("a.rs"));
        view.path = "missing.rs".into();
        view.navigate_file(false);
        assert_eq!(view.path, Path::new("c.rs"));
        view.set_files(Vec::new());
        view.navigate_file(true);
        view.navigate_file(false);
        assert!(view.sections.is_empty());
    }

    #[test]
    fn file_jump_keeps_live_document_but_cannot_snap_back_to_its_caret() {
        let mut view = multi_file_review();
        view.focus_column(1);
        let mut pane = CodePane::new("/repo/a.rs".into(), "working tree\n");
        pane.buffer.insert_text("dirty ");
        view.set_base_text(Path::new("a.rs"), "old\n".into());
        view.sync_live_editor(&mut pane);
        view.set_file_lines(
            Path::new("b.rs"),
            vec![
                DiffLine {
                    text: "destination".into(),
                    kind: DiffLineKind::Add,
                    line_number: Some(1),
                    old_line_number: None,
                    new_line_number: Some(1),
                };
                100
            ],
        );
        let text = pane.buffer.text();
        let cursor = (pane.buffer.cursor_line, pane.buffer.cursor_col);
        pane.buffer.follow_cursor = true;
        view.navigate_file(true);
        assert!(view.editing_path().is_none());
        assert!(view.take_edit_request().is_none());
        assert!(!view.reveal_edit_on_focus);
        assert_eq!(view.document_path(), Some(PathBuf::from("/repo/a.rs")));
        view.measure_sections(800.0, 1.0, 400.0);
        let scroll = view.scroll_top;
        assert!(scroll > 0.0);
        view.reveal_editor_cursor(&mut pane, Rect::new(0.0, 0.0, 800.0, 400.0), 1.0, 8.0);
        assert_eq!(view.scroll_top, scroll);
        assert_eq!(pane.buffer.text(), text);
        assert_eq!((pane.buffer.cursor_line, pane.buffer.cursor_col), cursor);
        view.focus_column(1);
        assert_eq!(view.take_edit_request().unwrap().path, Path::new("b.rs"));
    }

    #[test]
    fn historical_file_navigation_never_acquires_a_working_document() {
        let mut view = multi_file_review();
        view.commit_id = Some("abc123".into());
        view.focus_column(1);
        view.navigate_file(true);
        assert_eq!(view.path, Path::new("b.rs"));
        assert_eq!(view.commit_id.as_deref(), Some("abc123"));
        assert!(view.editing_path().is_none());
        assert!(view.document_path().is_none());
        assert!(view.take_edit_request().is_none());
        assert!(view.take_actions().is_empty());
    }

    #[test]
    fn horizontal_focus_walks_both_columns_before_leaving_review() {
        let mut view = review();
        assert_eq!(view.column_focused(), Some(1));
        assert!(view.move_column_focus(false));
        assert_eq!(view.column_focused(), Some(0));
        assert!(view.editing_path().is_none());
        assert!(!view.move_column_focus(false));
        assert!(view.move_column_focus(true));
        assert_eq!(view.column_focused(), Some(1));
        assert_eq!(view.editing_path(), Some(PathBuf::from("/repo/a.rs")));
        assert_eq!(view.take_edit_request().unwrap().line, 41);
        assert!(!view.move_column_focus(true));
    }

    #[test]
    fn return_to_working_tree_preserves_existing_owner_and_caret() {
        let mut view = review();
        view.focus_column(1);
        view.take_edit_request();
        view.focus_column(0);
        assert!(view.focus_column(1));
        assert!(
            view.take_edit_request().is_none(),
            "do not reset an existing canonical cursor on focus"
        );
        assert!(view.reveal_edit_on_focus);
        view.blur_columns();
        assert!(view.column_focused().is_none());
        assert!(view.editing_path().is_none());
        assert_eq!(view.document_path(), Some(PathBuf::from("/repo/a.rs")));
    }

    #[test]
    fn unified_mode_has_only_a_working_tree_focus_stop_and_deleted_rows_are_read_only() {
        let mut view = review();
        view.effective_split = false;
        assert!(view.focus_column(0));
        assert_eq!(view.column_focused(), Some(1));
        assert!(!view.move_column_focus(false));
        let mut view = review();
        view.set_file_lines(
            Path::new("a.rs"),
            vec![DiffLine {
                text: "deleted".into(),
                kind: DiffLineKind::Remove,
                line_number: Some(1),
                old_line_number: Some(1),
                new_line_number: None,
            }],
        );
        view.focus_column(1);
        assert!(view.take_edit_request().is_none());
        assert!(view.editing_path().is_none());
    }
}
