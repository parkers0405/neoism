use super::*;
use crate::panels::buffer_tabs::{BufferTabTarget, ChromePageKind};
use std::path::{Path, PathBuf};

fn change_block() -> neoism_protocol::git::GitChangeBlock {
    neoism_protocol::git::GitChangeBlock {
        old_start: 0,
        new_start: 0,
        old_lines: vec!["clean".into()],
        new_lines: vec!["dirty clean".into()],
    }
}

#[test]
fn block_save_gate_captures_payload_revision_and_never_queues_whole_file_stage() {
    let mut chrome = Chrome::<()>::new();
    chrome.git_diff_panel.open(Some("/repo".into()), None);
    let tab = chrome.buffer_tabs.open_path("/repo/a.rs".into());
    chrome.set_active_tab_index(tab);
    chrome.open_editor_file(tab, "/repo/a.rs", "clean\n");
    assert!(chrome.prepare_git_panel_block_stage(Path::new("a.rs"), change_block()));
    assert!(chrome.take_git_diff_block_save_stages().is_empty());
    chrome.code_pane_mut().unwrap().buffer.insert_text("dirty ");
    let revision = chrome.code_pane().unwrap().buffer.revision;
    assert!(!chrome.prepare_git_panel_block_stage(Path::new("a.rs"), change_block()));
    let mut other = change_block();
    other.new_start = 10;
    assert!(!chrome.prepare_git_panel_block_stage(Path::new("a.rs"), other));
    chrome.code_pane_mut().unwrap().buffer.insert_text("later ");
    assert!(chrome.take_git_diff_save_stages().is_empty());
    assert_eq!(
        chrome.take_git_diff_block_save_stages(),
        vec![("/repo".into(), "a.rs".into(), change_block(), revision,)]
    );
    assert!(chrome.take_git_diff_block_save_stages().is_empty());
}

#[test]
fn block_save_gate_refuses_parked_owners_and_drops_requests_from_another_repo() {
    let mut chrome = Chrome::<()>::new();
    chrome.git_diff_panel.open(Some("/repo".into()), None);
    let tab = chrome.buffer_tabs.open_path("/repo/a.rs".into());
    chrome.set_active_tab_index(tab);
    chrome.open_editor_file(tab, "/repo/a.rs", "clean\n");
    chrome.code_pane_mut().unwrap().buffer.insert_text("dirty ");
    chrome.open_editor_file(tab, "/repo/b.rs", "other\n");
    assert!(!chrome.prepare_git_panel_block_stage(Path::new("a.rs"), change_block()));
    assert!(chrome.take_git_diff_save_stages().is_empty());
    assert!(chrome.take_git_diff_block_save_stages().is_empty());
    assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
    assert!(!chrome.prepare_git_panel_block_stage(Path::new("a.rs"), change_block()));
    chrome
        .git_diff_panel
        .host_set_repo(Some("/other".into()), None);
    assert!(chrome.take_git_diff_block_save_stages().is_empty());
    assert!(chrome.code_pane().unwrap().is_dirty());
}

#[test]
fn provider_mutation_gate_covers_active_parked_and_markdown_owners() {
    let mut chrome = Chrome::<()>::new();
    chrome.git_diff_panel.open(Some("/repo".into()), None);
    let tab = chrome.buffer_tabs.open_path("/repo/a.rs".into());
    chrome.set_active_tab_index(tab);
    chrome.open_editor_file(tab, "/repo/a.rs", "clean\n");
    assert!(chrome.prepare_git_panel_stage(Path::new("a.rs")));
    chrome.code_pane_mut().unwrap().buffer.insert_text("dirty ");
    assert!(!chrome.prepare_git_panel_stage(Path::new("a.rs")));
    assert_eq!(
        chrome.take_git_diff_save_stages(),
        vec![("/repo".into(), "a.rs".into(), false)]
    );
    assert!(!chrome.prepare_git_panel_restore(Path::new("a.rs")));
    chrome.open_editor_file(tab, "/repo/b.rs", "other\n");
    assert!(!chrome.prepare_git_panel_stage(Path::new("a.rs")));
    assert!(chrome.take_git_diff_save_stages().is_empty());
    assert!(!chrome.prepare_git_panel_restore(Path::new("a.rs")));
    chrome.set_markdown_content(Some("# Draft\n".into()), Some("/repo/note.md"));
    chrome.markdown_pane_mut().unwrap().insert_text("unsaved ");
    assert!(!chrome.prepare_git_panel_stage(Path::new("note.md")));
    assert!(!chrome.prepare_git_panel_restore(Path::new("note.md")));
    assert!(chrome.take_git_diff_save_stages().is_empty());
}

#[test]
fn inline_editor_reuses_the_canonical_active_and_parked_buffer_without_tabs_or_reseed() {
    let mut chrome = Chrome::<()>::new();
    chrome.open_editor_file(0, "/repo/a.rs", "original\n");
    chrome
        .code_pane_mut()
        .unwrap()
        .buffer
        .insert_text("unsaved ");
    let dirty = chrome.code_pane().unwrap().buffer.text_for_disk();
    let cursor = chrome.code_pane().unwrap().buffer.cursor();
    let tabs = chrome.buffer_tabs.tabs().len();
    assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
    assert_eq!(chrome.code_pane().unwrap().buffer.text_for_disk(), dirty);
    chrome.open_editor_file(1, "/repo/b.rs", "other\n");
    assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
    let pane = chrome.code_pane().unwrap();
    assert_eq!(pane.buffer.text_for_disk(), dirty);
    assert_eq!(pane.buffer.cursor(), cursor);
    assert!(pane.is_dirty());
    assert_eq!(chrome.buffer_tabs.tabs().len(), tabs);
    assert!(!chrome.activate_git_diff_editor(Path::new("/repo/missing.rs")));
    assert_eq!(chrome.code_pane().unwrap().path, Path::new("/repo/a.rs"));
}

#[test]
fn git_diff_routes_are_scoped_and_do_not_claim_editable_or_markdown_surfaces() {
    let mut chrome = Chrome::<()>::new();
    let root = PathBuf::from("/repo");
    let path = root.join("README.md");
    chrome.ensure_git_diff_view(11, root.clone(), path.clone());
    chrome.ensure_git_diff_view(
        12,
        PathBuf::from("/other"),
        PathBuf::from("/other/README.md"),
    );
    chrome.buffer_tabs.open_git_diff(11, &path);
    let ix = chrome
        .buffer_tabs
        .tabs()
        .iter()
        .position(|tab| {
            tab.chrome_page.is_some_and(|page| {
                page.kind == ChromePageKind::GitDiff && page.route_id == 11
            })
        })
        .unwrap();
    chrome.set_active_tab_index(ix);
    assert_eq!(chrome.active_chrome_page(), Some(ChromePageKind::GitDiff));
    assert!(!chrome.is_terminal_tab_active());
    assert_eq!(chrome.active_editor_pane_kind(), None);
    assert!(matches!(
        chrome.buffer_tabs.target_at(ix),
        Some(BufferTabTarget::ChromePage(_))
    ));
    assert_eq!(chrome.git_diff_views.len(), 2);
    chrome.git_diff_panel.close();
    assert_eq!(chrome.active_chrome_page(), Some(ChromePageKind::GitDiff));
    assert_eq!(chrome.git_diff_views[&11].path, Path::new("README.md"));
    chrome.retain_git_diff_views(&[11]);
    assert_eq!(chrome.git_diff_views.len(), 1);
    assert!(!chrome.git_diff_views.contains_key(&12));
}

#[test]
fn replay_preserves_diff_view_identity_but_replaces_reused_route_metadata() {
    let mut chrome = Chrome::<()>::new();
    chrome.ensure_git_diff_view(7, "/repo".into(), "/repo/deleted.rs".into());
    chrome.git_diff_view_generations.insert(7, 42);
    chrome.ensure_git_diff_view(7, "/repo".into(), "/repo/deleted.rs".into());
    assert_eq!(chrome.git_diff_view_generations.get(&7), Some(&42));
    assert_eq!(chrome.git_diff_views.len(), 1);
    chrome.ensure_git_diff_view(7, "/repo".into(), "/repo/new.rs".into());
    // A changed selected-path hint in a JS tab-list replay is not navigation.
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("deleted.rs"));
    assert_eq!(chrome.git_diff_view_generations.get(&7), Some(&42));
    chrome.reveal_git_diff_file(7, Path::new("/repo/new.rs"));
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("new.rs"));
    chrome.ensure_git_diff_view(7, "/repo".into(), "/repo/deleted.rs".into());
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("new.rs"));
    assert_eq!(chrome.git_diff_view_generations.get(&7), Some(&42));
    // A reused route for a different repository really does replace the view.
    chrome.ensure_git_diff_view(7, "/other".into(), "/other/new.rs".into());
    assert_eq!(chrome.git_diff_views[&7].root, Path::new("/other"));
    assert!(!chrome.git_diff_view_generations.contains_key(&7));
}

fn sidebar_review_fixture() -> Chrome<()> {
    use crate::panels::git_diff::{FileChange, FileStatus};
    let mut chrome = Chrome::<()>::new();
    chrome.git_diff_panel.open(Some("/repo".into()), None);
    chrome.git_diff_panel.host_set_files(
        ["a.rs", "nested/b.rs", "z.rs"]
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
    chrome.git_diff_panel.focus_files_section();
    chrome.git_diff_panel.set_focused(true);
    chrome.git_diff_panel.select_first_file();
    chrome.git_diff_panel.select_file(0);
    chrome.ensure_git_diff_view(7, "/repo".into(), "a.rs".into());
    let ix = chrome.buffer_tabs.open_git_diff(7, Path::new("a.rs"));
    chrome.set_active_tab_index(ix);
    chrome
}

fn sidebar_arrow(chrome: &mut Chrome<()>, down: bool) {
    use crate::event::*;
    assert!(chrome.handle_side_panel_key(&KeyDescriptor {
        physical: PhysicalKey(0),
        logical: LogicalKey::Named(if down {
            NamedKey::ArrowDown
        } else {
            NamedKey::ArrowUp
        }),
        state: KeyState::Pressed,
        modifiers: Modifiers::empty(),
        repeat: false,
    }));
}

#[test]
fn sidebar_vim_motion_reveals_without_opening_or_focusing_review() {
    use crate::event::*;
    let mut chrome = sidebar_review_fixture();
    let tab = chrome.active_tab_index;
    let tabs = chrome.buffer_tabs.tabs().len();
    for (key, path) in [("j", "z.rs"), ("k", "a.rs")] {
        assert!(chrome.handle_side_panel_key(&KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Character(key.into()),
            state: KeyState::Pressed,
            modifiers: Modifiers::empty(),
            repeat: false,
        }));
        assert_eq!(chrome.git_diff_views[&7].path, Path::new(path));
        assert!(chrome.git_diff_panel.is_focused());
        assert_eq!(chrome.git_diff_views[&7].column_focused(), None);
        assert_eq!(chrome.active_tab_index, tab);
        assert_eq!(chrome.buffer_tabs.tabs().len(), tabs);
        assert!(chrome.drain_git_diff_opens().is_empty());
    }
}

#[test]
fn pinned_sidebar_does_not_consume_canonical_review_insert_escape() {
    use crate::editor::code::{CodeInputMode, CodeMode};
    use crate::event::*;
    let mut chrome = sidebar_review_fixture();
    chrome.open_editor_file(0, "/repo/a.rs", "aéz\n");
    assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
    let view = chrome.git_diff_views.get_mut(&7).unwrap();
    view.set_file_lines(
        Path::new("a.rs"),
        vec![crate::widgets::diff_card::DiffLine {
            text: "aéz".into(),
            kind: crate::widgets::diff_card::DiffLineKind::Add,
            line_number: Some(1),
            old_line_number: None,
            new_line_number: Some(1),
        }],
    );
    assert!(view.focus_column(1));
    chrome.git_diff_panel.set_focused(false);
    let pane = chrome.code_pane_mut().unwrap();
    pane.input_mode = CodeInputMode::Vim;
    pane.buffer.mode = CodeMode::Insert;
    pane.buffer.set_cursor_position(0, 3, false);
    let escape = KeyDescriptor {
        physical: PhysicalKey(0),
        logical: LogicalKey::Named(NamedKey::Escape),
        state: KeyState::Pressed,
        modifiers: Modifiers::empty(),
        repeat: false,
    };
    // The host's editor_key/dispatch_code_key must receive Escape, not the
    // sidebar or the read-only review-page key handler.
    assert!(!chrome.handle_side_panel_key(&escape));
    assert!(!chrome.handle_chrome_page_event(&UiEvent::Key(escape)));
    assert!(chrome.git_diff_panel.is_visible());
    assert_eq!(
        chrome.active_editor_pane_kind(),
        Some(super::content::EditorPaneKind::Code)
    );
    assert_eq!(chrome.code_pane().unwrap().buffer.cursor_line, 0);
    assert_eq!(chrome.code_pane().unwrap().buffer.cursor_col, 3);
    assert_eq!(chrome.code_pane().unwrap().buffer.mode, CodeMode::Insert);
}

#[test]
fn sidebar_arrows_reveal_same_review_and_keep_focus_identity_and_source() {
    let mut chrome = sidebar_review_fixture();
    chrome.open_editor_file(0, "/repo/a.rs", "original\n");
    chrome.code_pane_mut().unwrap().buffer.insert_text("dirty ");
    let text = chrome.code_pane().unwrap().buffer.text_for_disk();
    let cursor = chrome.code_pane().unwrap().buffer.cursor();
    let tab = chrome.active_tab_index;
    let tabs = chrome.buffer_tabs.tabs().len();
    chrome.git_diff_view_generations.insert(7, 42);
    sidebar_arrow(&mut chrome, true);
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("z.rs"));
    sidebar_arrow(&mut chrome, false);
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("a.rs"));
    assert!(chrome.git_diff_panel.is_focused());
    assert_eq!(chrome.active_tab_index, tab);
    assert_eq!(chrome.buffer_tabs.tabs().len(), tabs);
    assert_eq!(chrome.git_diff_views.len(), 1);
    assert_eq!(chrome.git_diff_view_generations[&7], 42);
    assert!(chrome.drain_git_diff_opens().is_empty());
    assert_eq!(chrome.code_pane().unwrap().buffer.text_for_disk(), text);
    assert_eq!(chrome.code_pane().unwrap().buffer.cursor(), cursor);
    assert!(chrome.code_pane().unwrap().is_dirty());
    assert!(chrome.code_pane_mut().unwrap().buffer.undo());
    assert_eq!(
        chrome.code_pane().unwrap().buffer.text_for_disk(),
        "original\n"
    );
}

#[test]
fn sidebar_arrows_skip_collapsed_leaves_and_do_not_reveal_hidden_edge_selection() {
    let mut chrome = sidebar_review_fixture();
    // Row zero is Changes; row one is the nested directory.
    chrome.git_diff_panel.toggle_folder(1);
    sidebar_arrow(&mut chrome, true);
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("z.rs"));
    sidebar_arrow(&mut chrome, false);
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("a.rs"));
    sidebar_arrow(&mut chrome, false);
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("a.rs"));
    assert!(chrome.git_diff_panel.is_focused());
    assert!(chrome.drain_git_diff_opens().is_empty());
}

#[test]
fn sidebar_reveal_rejects_other_root_history_missing_and_inactive_reviews() {
    let mut chrome = sidebar_review_fixture();
    chrome.git_diff_panel.select_last_file();
    chrome.git_diff_views.get_mut(&7).unwrap().root = "/other".into();
    assert!(!chrome.reveal_selected_git_diff());
    chrome.git_diff_views.get_mut(&7).unwrap().root = "/repo".into();
    chrome.git_diff_views.get_mut(&7).unwrap().commit_id = Some("abc".into());
    assert!(!chrome.reveal_selected_git_diff());
    chrome.git_diff_views.get_mut(&7).unwrap().commit_id = None;
    let ordinary = chrome.buffer_tabs.open_path("/repo/unrelated.rs".into());
    chrome.set_active_tab_index(ordinary);
    let tabs = chrome.buffer_tabs.tabs().len();
    sidebar_arrow(&mut chrome, false);
    assert_eq!(chrome.active_tab_index, ordinary);
    assert_eq!(chrome.git_diff_views[&7].path, Path::new("a.rs"));
    chrome.retain_git_diff_views(&[]);
    sidebar_arrow(&mut chrome, true);
    assert!(chrome.git_diff_views.is_empty());
    assert!(chrome.drain_git_diff_opens().is_empty());
    assert_eq!(chrome.buffer_tabs.tabs().len(), tabs);
    assert!(chrome.git_diff_panel.is_focused());
}
