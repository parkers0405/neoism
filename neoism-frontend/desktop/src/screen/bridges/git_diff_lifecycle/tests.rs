use super::*;
use neoism_ui::panels::buffer_tabs::BufferTabs;

#[test]
fn review_close_promotes_dirty_owner_without_touching_its_text_or_history() {
    let path = PathBuf::from("/nonexistent/neoism-review-lifecycle.rs");
    let mut owner = neoism_ui::editor::code::CodePane::load_with_source(
        path.clone(),
        neoism_ui::services::FileOpenSource::LocalOnly,
    );
    let baseline = owner.buffer.text();
    owner.buffer.insert_text("unsaved canonical text");
    let edited = owner.buffer.text();
    let mut tabs = BufferTabs::<()>::new();
    tabs.ensure_terminal_tab();
    let review = tabs.open_git_diff(17, &path);
    assert!(!tabs
        .tabs()
        .iter()
        .any(|tab| matches!(tab.target(), Some(BufferTabTarget::File(_)))));
    register_review_dirty_paths(&mut tabs, &[path.clone()]);
    assert_eq!(tabs.active(), review);
    let (closed, next) = tabs.close_at(review);
    assert!(
        matches!(closed, Some(BufferTabTarget::ChromePage(page)) if page.route_id == 17)
    );
    assert_eq!(next, Some(BufferTabTarget::File(path.clone())));
    assert_eq!(tabs.target_at(tabs.active()), next);
    assert!(tabs
        .tabs()
        .iter()
        .any(|tab| tab.path.as_deref() == Some(path.as_path()) && tab.modified));
    assert_eq!(owner.buffer.text(), edited);
    owner.buffer.undo();
    assert_eq!(owner.buffer.text(), baseline);
}

#[test]
fn promotion_is_idempotent_and_registers_every_dirty_document() {
    let a = PathBuf::from("/repo/a.rs");
    let b = PathBuf::from("/repo/b.rs");
    let mut tabs = BufferTabs::<()>::new();
    tabs.ensure_terminal_tab();
    tabs.open_path(a.clone());
    let review = tabs.open_git_diff(21, &a);
    let paths = [a.clone(), b.clone()];
    register_review_dirty_paths(&mut tabs, &paths);
    register_review_dirty_paths(&mut tabs, &paths);
    assert_eq!(tabs.active(), review);
    assert_eq!(tabs.tabs().len(), 4);
    tabs.close_at(review);
    for path in paths {
        assert_eq!(
            tabs.tabs()
                .iter()
                .filter(|tab| tab.path.as_deref() == Some(path.as_path()) && tab.modified)
                .count(),
            1
        );
    }
    assert!(tabs.target_at(tabs.active()).is_some() || tabs.active_is_terminal());
}

#[test]
fn review_dirty_dot_targets_route_not_file_path_and_clears_after_save() {
    let path = PathBuf::from("/repo/a.rs");
    let mut tabs = BufferTabs::<()>::new();
    tabs.ensure_terminal_tab();
    let file = tabs.open_path(path.clone());
    let review = tabs.open_git_diff(31, &path);
    let history = tabs.open_git_diff(32, &path);
    set_review_page_flags(&mut tabs, &[(31, true), (32, false)]);
    assert!(tabs.tabs()[review].modified);
    assert!(!tabs.tabs()[history].modified);
    assert!(!tabs.tabs()[file].modified);
    set_review_page_flags(&mut tabs, &[(31, false), (32, false)]);
    assert!(!tabs.tabs()[review].modified);
    assert_eq!(tabs.active(), history);
}
