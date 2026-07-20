// Tests moved from screen/mod.rs.

use super::*;
use neoism_terminal_core::crosswords::square::LineLength;
use neoism_ui::editor::scroll_model::raw_scroll_has_room;
use neoism_ui::editor::selection_model::post_process_hyperlink_uri;

#[test]
fn absolute_row_mapping_round_trips_visible_indices() {
    // history_size + line = absolute row. When scrolled back by 4,
    // visible index 2 maps to terminal line -2 and abs 98.
    let history_size = 100;
    let display_offset = 4;
    let abs = 98;

    assert_eq!(line_for_absolute_row(abs, history_size), Line(-2));
    assert_eq!(
        visible_index_for_absolute_row(abs, history_size, display_offset),
        Some(2)
    );
}

#[test]
fn composed_cursor_row_skips_virtual_chrome_rows() {
    let sources = [None, None, Some(42), Some(43), None, Some(44)];

    assert_eq!(composed_display_row_for_abs(&sources, 42), Some(2));
    assert_eq!(composed_display_row_for_abs(&sources, 44), Some(5));
    assert_eq!(composed_display_row_for_abs(&sources, 45), None);
}

#[test]
fn composer_prompt_row_trim_drops_matching_row_even_when_shell_painted_it() {
    fn row_with_text(text: &str) -> Row<Square> {
        let mut row = Row::new(8);
        for (idx, ch) in text.chars().enumerate() {
            row.inner[idx] = Square::from_char(ch);
        }
        row
    }

    let mut rows = vec![row_with_text("out"), Row::new(8), row_with_text("next")];
    let mut sources = vec![10, 11, 12];

    drop_composer_owned_prompt_row(&mut rows, &mut sources, Some(11));
    assert_eq!(sources, vec![10, 12]);

    drop_composer_owned_prompt_row(&mut rows, &mut sources, Some(10));
    assert_eq!(sources, vec![12]);
}

#[test]
fn block_scroll_cursor_survives_raw_viewport_mismatch() {
    let existing = crate::terminal::scroll::BlockScrollCursor {
        raw_top_abs: 91,
        chrome_row: 1,
    };

    assert_eq!(block_scroll_cursor_or_anchor(Some(existing), 89), existing);
    assert_eq!(
        block_scroll_cursor_or_anchor(None, 89),
        crate::terminal::scroll::BlockScrollCursor {
            raw_top_abs: 89,
            chrome_row: 0,
        }
    );
}

#[test]
fn raw_scroll_room_detects_mid_history_recovery() {
    assert!(raw_scroll_has_room(1, 4, 10));
    assert!(raw_scroll_has_room(-1, 4, 10));
    assert!(!raw_scroll_has_room(1, 10, 10));
    assert!(!raw_scroll_has_room(-1, 0, 10));
    assert!(!raw_scroll_has_room(0, 4, 10));
}

#[test]
fn passthrough_shell_detection_handles_raw_remote_sessions() {
    assert!(!starts_passthrough_session("bash"));
    assert!(!starts_passthrough_session("zsh"));
    assert!(!starts_passthrough_session("fish"));
    assert!(!starts_passthrough_session("nix-shell"));
    assert!(starts_passthrough_session("sh"));
    assert!(!starts_passthrough_session("bash script.sh"));
    assert!(starts_passthrough_session("ssh devbox"));
    assert!(starts_passthrough_session("ssh -t devbox"));
    assert!(starts_passthrough_session("/usr/bin/ssh devbox"));
    assert!(starts_passthrough_session("mosh devbox"));
    assert!(!starts_passthrough_session("python"));
    assert!(ends_passthrough_session("exit"));
    assert!(ends_passthrough_session(" logout "));
    assert!(!ends_passthrough_session("exit now"));
}

#[test]
fn git_state_event_filter_ignores_lock_churn() {
    let event =
        notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("/repo/.git/index.lock"));

    assert!(!git_state_event_relevant(&event));
}

#[test]
fn git_state_event_filter_ignores_pathless_churn() {
    let event =
        notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Any));

    assert!(!git_state_event_relevant(&event));
}

#[test]
fn git_state_event_filter_accepts_stable_git_state() {
    let index = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(PathBuf::from("/repo/.git/index"));
    let head = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(PathBuf::from("/repo/.git/HEAD"));
    let branch_ref = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(PathBuf::from("/repo/.git/refs/heads/main"));

    assert!(git_state_event_relevant(&index));
    assert!(git_state_event_relevant(&head));
    assert!(git_state_event_relevant(&branch_ref));
}

#[test]
fn file_tree_fs_event_filter_ignores_noisy_paths() {
    let root = PathBuf::from("/repo");
    let git = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join(".git/index"));
    let target = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join("target/debug/build/output"));
    let claude = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join(".claude/worktrees/agent-a123/src/main.rs"));
    let swap = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join("src/main.rs.swp"));

    assert!(!file_tree_fs_event_relevant(&root, &git));
    assert!(!file_tree_fs_event_relevant(&root, &target));
    assert!(!file_tree_fs_event_relevant(&root, &claude));
    assert!(!file_tree_fs_event_relevant(&root, &swap));
}

#[test]
fn file_tree_fs_event_filter_accepts_worktree_changes() {
    let root = PathBuf::from("/repo");
    let modified = notify::Event::new(notify::EventKind::Modify(
        notify::event::ModifyKind::Data(notify::event::DataChange::Any),
    ))
    .add_path(root.join("src/main.rs"));
    let created =
        notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(root.join("src/new_file.rs"));
    let write_closed = notify::Event::new(notify::EventKind::Access(
        notify::event::AccessKind::Close(notify::event::AccessMode::Write),
    ))
    .add_path(root.join("src/saved.rs"));

    assert!(file_tree_fs_event_relevant(&root, &modified));
    assert!(file_tree_fs_event_relevant(&root, &created));
    assert!(file_tree_fs_event_relevant(&root, &write_closed));
}

#[test]
fn test_post_process_hyperlink_uri() {
    // Test removing trailing parenthesis
    assert_eq!(
        post_process_hyperlink_uri("https://example.com)"),
        "https://example.com"
    );

    // Test removing trailing comma
    assert_eq!(
        post_process_hyperlink_uri("https://example.com,"),
        "https://example.com"
    );

    // Test removing trailing period
    assert_eq!(
        post_process_hyperlink_uri("https://example.com."),
        "https://example.com"
    );

    // Test handling balanced parentheses (should keep them)
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path(with)parens"),
        "https://example.com/path(with)parens"
    );

    // Test handling unbalanced parentheses
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path)"),
        "https://example.com/path"
    );

    // Test handling multiple trailing delimiters
    assert_eq!(
        post_process_hyperlink_uri("https://example.com.'),"),
        "https://example.com"
    );

    // Test markdown-style URLs
    assert_eq!(
        post_process_hyperlink_uri("https://example.com)"),
        "https://example.com"
    );

    // Test handling unbalanced brackets
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path]"),
        "https://example.com/path"
    );

    // Test balanced brackets (should keep them)
    assert_eq!(
        post_process_hyperlink_uri("https://example.com/path[with]brackets"),
        "https://example.com/path[with]brackets"
    );
}
