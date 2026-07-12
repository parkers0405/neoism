
use super::super::chrome::{
    compose_block_chrome, compose_block_chrome_window, COMMAND_BLOCK_CHROME_ROWS,
};
use super::super::completion::completion_candidates;
use super::*;
use crate::TerminalShellKind;
use neoism_terminal_core::crosswords::grid::row::Row;
use neoism_terminal_core::crosswords::square::Square;
use neoism_terminal_core::crosswords::ShellPromptState;
use std::collections::BTreeSet;

#[test]
fn command_payload_sends_plain_command_without_control_prefix() {
    let payload = TerminalShellKind::Bash.command_payload("echo ok", true);
    assert_eq!(payload, b"echo ok\n");
}

#[test]
fn command_payload_strips_c0_controls_except_newline_and_tab() {
    let payload =
        TerminalShellKind::Bash.command_payload("echo\x04 ok\tthere\nnext", true);
    assert_eq!(payload, b"echo ok\tthere\nnext\n");
}

#[test]
fn command_payload_normalizes_multiline_paste_before_submit() {
    let payload =
        TerminalShellKind::Bash.command_payload("echo one\r\necho \x1b[31mtwo\x03", true);
    assert_eq!(payload, b"echo one\necho two\n");
    assert!(!payload.windows(2).any(|window| window == b"\\n"));
    assert!(!payload.contains(&b'\x1b'));
    assert!(!payload.contains(&b'\r'));
}

#[test]
fn submit_sanitizes_hidden_control_bytes() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("echo one\r\necho \x1b[31mtwo\x03");

    assert_eq!(input.submit_with_context(None, None), "echo one\necho two");
}

#[test]
fn insert_paste_normalizes_line_endings_and_controls() {
    let mut input = TerminalInputBuffer::default();
    input.insert_paste("echo one\r\necho \x1b[31mtwo\x03");

    assert_eq!(input.text(), "echo one\necho two");
}

#[test]
fn insert_paste_trims_trailing_blank_rows_only() {
    let mut input = TerminalInputBuffer::default();
    input.insert_paste("echo one\r\n\r\necho two\r\n\r\n");

    assert_eq!(input.text(), "echo one\n\necho two");
}

#[test]
fn readline_word_delete_updates_buffer() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("echo hello world");
    input.delete_previous_word();
    assert_eq!(input.text(), "echo hello ");
}

#[test]
fn input_insert_preserves_multiline_paste_text() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("echo one\r\necho two\tok\x1b[31m");

    assert_eq!(input.text(), "echo one\necho two\tok");
}

#[test]
fn multiline_arrow_navigation_keeps_column() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("alpha\nbeta\ncharlie");

    assert!(input.move_visual_up());
    assert_eq!(input.cursor, "alpha\nbeta".len());
    assert!(input.move_visual_up());
    assert_eq!(input.cursor, "alpha".len());
    assert!(!input.move_visual_up());
    assert!(input.move_visual_down());
    assert_eq!(input.cursor, "alpha\nbeta".len());

    input.cursor = "alpha\nbe".len();
    input.desired_visual_column = None;
    assert!(input.move_visual_up());
    assert_eq!(input.cursor, "al".len());
    assert!(input.move_visual_down());
    assert_eq!(input.cursor, "alpha\nbe".len());
}

#[test]
fn wrapped_visual_arrow_navigation_tracks_soft_lines() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("abcdefghijkl");
    let ranges = [(0, 5), (5, 10), (10, 12)];

    assert!(input.move_visual_up_in_ranges(&ranges));
    assert_eq!(input.cursor, 7);
    assert!(input.move_visual_up_in_ranges(&ranges));
    assert_eq!(input.cursor, 2);
    assert!(!input.move_visual_up_in_ranges(&ranges));
    assert!(input.move_visual_down_in_ranges(&ranges));
    assert_eq!(input.cursor, 7);
    input.backspace();
    assert_eq!(input.text(), "abcdefhijkl");
    assert_eq!(input.cursor, 6);
}

#[test]
fn multiline_home_end_stay_on_current_line() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("one\ntwo\nthree");
    input.cursor = "one\nt".len();

    input.move_home();
    assert_eq!(input.cursor, "one\n".len());
    input.move_end();
    assert_eq!(input.cursor, "one\ntwo".len());
}

#[test]
fn history_previous_prefers_most_recent_prefix_match() {
    let mut input = TerminalInputBuffer::default();
    input.push_history("cargo test".to_string());
    input.push_history("docker ps".to_string());
    input.push_history("git status".to_string());
    input.push_history("docker compose up".to_string());
    input.insert_str("docker");

    assert!(input.history_previous());
    assert_eq!(input.text(), "docker compose up");
    assert!(input.history_previous());
    assert_eq!(input.text(), "docker ps");
    assert!(input.history_next());
    assert_eq!(input.text(), "docker compose up");
    assert!(input.history_next());
    assert_eq!(input.text(), "docker");
}

#[test]
fn history_previous_uses_text_before_cursor_as_fuzzy_query() {
    let mut input = TerminalInputBuffer::default();
    input.push_history("cargo test -p neoism-ui".to_string());
    input.push_history("cd neoism-frontend/shared".to_string());
    input.insert_str("front --keep-this-draft");
    input.cursor = "front".len();

    assert!(input.history_previous());
    assert_eq!(input.text(), "cd neoism-frontend/shared");
    assert!(input.history_next());
    assert_eq!(input.text(), "front --keep-this-draft");
}

#[test]
fn ctrl_r_history_picker_uses_fuzzy_matches() {
    let mut input = TerminalInputBuffer::default();
    input.push_history("cargo test -p neoism-ui".to_string());
    input.push_history("git switch feature/demo".to_string());
    input.insert_str("sw feat");

    assert!(input.open_history_picker());
    assert_eq!(input.text(), "git switch feature/demo");
    assert!(input.completion_menu_active());
    assert!(input
        .completion_detail()
        .is_some_and(|detail| detail.contains("sw feat")));
}

#[test]
fn ctrl_f_favorite_picker_uses_fuzzy_matches() {
    let mut input = TerminalInputBuffer::default();
    assert_eq!(
        input.toggle_favorite_command("cargo test -p neoism-ui"),
        Some(true)
    );
    assert_eq!(
        input.toggle_favorite_command("git switch feature/demo"),
        Some(true)
    );
    input.insert_str("sw feat");

    assert!(input.open_favorite_picker());
    assert_eq!(input.text(), "git switch feature/demo");
    assert!(input.completion_menu_active());
    assert!(input
        .completion_detail()
        .is_some_and(|detail| detail.contains("sw feat")));
}

#[test]
fn history_recall_strips_trailing_newline() {
    let mut input = TerminalInputBuffer::default();
    input.push_history("./target/debug/neoism\n".to_string());

    assert!(input.history_previous());
    assert_eq!(input.text(), "./target/debug/neoism");
}

#[test]
fn persistent_history_round_trips_recent_entries() {
    let path = std::env::temp_dir().join(format!(
        "neoism-history-test-{}-{}",
        std::process::id(),
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut input = TerminalInputBuffer::default();
    input.enable_persistent_history(path.clone());
    input.push_history("docker compose up".to_string());
    input.push_history("cargo test".to_string());

    let mut reloaded = TerminalInputBuffer::default();
    reloaded.enable_persistent_history(path.clone());

    assert_eq!(
        reloaded.history,
        vec!["docker compose up".to_string(), "cargo test".to_string()]
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn persistent_favorites_round_trips_recent_entries() {
    let path = std::env::temp_dir().join(format!(
        "neoism-favorites-test-{}-{}",
        std::process::id(),
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut input = TerminalInputBuffer::default();
    input.enable_persistent_favorites(path.clone());
    assert_eq!(
        input.toggle_favorite_command("docker compose up"),
        Some(true)
    );
    assert_eq!(input.toggle_favorite_command("cargo test"), Some(true));

    let mut reloaded = TerminalInputBuffer::default();
    reloaded.enable_persistent_favorites(path.clone());

    assert_eq!(
        reloaded.favorite_commands,
        vec!["docker compose up".to_string(), "cargo test".to_string()]
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn persistent_history_load_strips_trailing_newlines() {
    let path = std::env::temp_dir().join(format!(
        "neoism-history-newline-test-{}-{}",
        std::process::id(),
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b"./target/debug/neoism\n\0ls\0").unwrap();

    let mut input = TerminalInputBuffer::default();
    input.enable_persistent_history(path.clone());

    assert_eq!(
        input.history,
        vec!["./target/debug/neoism".to_string(), "ls".to_string()]
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn zsh_history_loads_extended_history_format() {
    let path = std::env::temp_dir().join(format!(
        "neoism-zsh-history-test-{}-{}",
        std::process::id(),
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b": 1700000000:0;git status\nls -la\n").unwrap();

    let mut input = TerminalInputBuffer::default();
    input.enable_zsh_history(path.clone());

    assert_eq!(
        input.history,
        vec!["git status".to_string(), "ls -la".to_string()]
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn zsh_history_does_not_duplicate_native_shell_writes() {
    let path = std::env::temp_dir().join(format!(
        "neoism-zsh-history-no-append-test-{}-{}",
        std::process::id(),
        web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, b": 1700000000:0;git status\n").unwrap();

    let mut input = TerminalInputBuffer::default();
    input.enable_zsh_history(path.clone());
    input.push_history("cargo test".to_string());

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        ": 1700000000:0;git status\n"
    );
    assert_eq!(input.history.last().map(String::as_str), Some("cargo test"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn finished_clear_block_is_dropped_when_next_command_submits() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("clear");
    input.submit_with_context(None, Some(10));
    input.sync_shell_state(ShellPromptState {
        awaiting_command: false,
        running_command: true,
        last_exit_code: None,
    });
    input.sync_shell_state(ShellPromptState {
        awaiting_command: true,
        running_command: false,
        last_exit_code: Some(0),
    });

    input.insert_str("ls");
    input.submit_with_context(None, Some(20));

    let commands = input
        .command_block_snapshots()
        .into_iter()
        .map(|block| block.command)
        .collect::<Vec<_>>();
    assert_eq!(commands, vec!["ls"]);
}

#[test]
fn running_clear_block_is_dropped_when_prompt_returns() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("ls");
    input.submit_with_context(None, Some(10));
    input.sync_shell_state(ShellPromptState {
        awaiting_command: true,
        running_command: false,
        last_exit_code: Some(0),
    });

    input.insert_str("clear");
    input.submit_with_context(None, Some(20));
    input.clear_previous_blocks_for_active_command();

    let commands = input
        .command_block_snapshots()
        .into_iter()
        .map(|block| block.command)
        .collect::<Vec<_>>();
    assert_eq!(commands, vec!["clear"]);

    assert!(!input.sync_shell_state(ShellPromptState {
        awaiting_command: false,
        running_command: true,
        last_exit_code: None,
    }));
    assert!(input.sync_shell_state(ShellPromptState {
        awaiting_command: true,
        running_command: false,
        last_exit_code: Some(0),
    }));
    assert!(input.command_block_snapshots().is_empty());
}

#[test]
fn passthrough_session_clears_when_parent_prompt_returns() {
    let mut input = TerminalInputBuffer::default();
    input.set_passthrough_session_active(true);
    input.sync_shell_state(ShellPromptState {
        awaiting_command: true,
        running_command: false,
        last_exit_code: Some(0),
    });

    assert!(!input.passthrough_session_active());
}

#[test]
fn cd_tab_completion_is_case_insensitive_and_directory_only() {
    let root =
        std::env::temp_dir().join(format!("neoism-tab-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("GitHub")).unwrap();
    std::fs::write(root.join("GitFile"), b"not a cd target").unwrap();

    let mut input = TerminalInputBuffer::default();
    input.insert_str("cd git");

    assert!(input.complete_or_accept(Some(&root)));
    assert_eq!(input.submit(), "cd GitHub/");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cd_tab_completion_matches_path_segments_like_zsh() {
    let root = std::env::temp_dir()
        .join(format!("neoism-tab-segment-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("neoism-frontend")).unwrap();
    std::fs::create_dir_all(root.join("neoism-backend")).unwrap();

    let mut input = TerminalInputBuffer::default();
    input.insert_str("cd front");

    assert!(input.complete_or_accept(Some(&root)));
    assert_eq!(input.submit(), "cd neoism-frontend/");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completion_arrows_cycle_visible_menu() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-arrow-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("alpha")).unwrap();
    std::fs::create_dir_all(root.join("beta")).unwrap();

    let mut input = TerminalInputBuffer::default();
    input.insert_str("cd ");

    assert!(input.complete_or_accept(Some(&root)));
    assert_eq!(input.text(), "cd alpha/");
    assert!(input.completion_menu_active());
    assert!(input.completion_items()[0].contains('\u{f07b}'));

    assert!(input.completion_next());
    assert_eq!(input.text(), "cd beta/");

    assert!(input.completion_previous());
    assert_eq!(input.text(), "cd alpha/");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn dismiss_completion_menu_keeps_completed_text() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-dismiss-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("alpha")).unwrap();
    std::fs::create_dir_all(root.join("beta")).unwrap();

    let mut input = TerminalInputBuffer::default();
    input.insert_str("cd ");
    assert!(input.complete_or_accept(Some(&root)));
    assert!(input.completion_menu_active());
    assert!(input.dismiss_completion_menu());
    assert_eq!(input.text(), "cd alpha/");
    assert!(!input.completion_menu_active());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completion_selection_stays_visible_past_old_display_limit() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-long-list-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for idx in 0..12 {
        std::fs::create_dir_all(root.join(format!("item{idx:02}"))).unwrap();
    }

    let mut input = TerminalInputBuffer::default();
    input.insert_str("cd ");

    assert!(input.complete_or_accept(Some(&root)));
    for _ in 0..12 {
        assert!(input.completion_next());
    }

    assert_eq!(input.text(), "cd item11/");
    assert_eq!(input.completion_items().len(), 12);
    assert_eq!(
        input
            .completion_items()
            .iter()
            .position(|item| item.starts_with('>')),
        Some(11)
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn command_position_completion_includes_local_paths() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-command-path-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("local-file"), b"ok").unwrap();

    let candidates = completion_candidates("", "", true, false, Some(&root));

    assert!(candidates
        .iter()
        .any(|candidate| candidate.replacement == "local-file"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn path_completion_hides_dot_entries_until_dot_prefix() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-dotfile-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agents")).unwrap();
    std::fs::create_dir_all(root.join("workspace")).unwrap();

    let bare = completion_candidates("", "cd ", false, true, Some(&root));
    assert!(bare
        .iter()
        .any(|candidate| candidate.replacement == "workspace/"));
    assert!(!bare
        .iter()
        .any(|candidate| candidate.replacement == ".agents/"));

    let dot = completion_candidates(".", "cd ", false, true, Some(&root));
    assert!(dot
        .iter()
        .any(|candidate| candidate.replacement == ".agents/"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cd_tab_does_not_accept_history_only_path_suggestion() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-cd-history-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let mut input = TerminalInputBuffer::default();
    input.push_history("cd missing-from-old-project".to_string());
    input.insert_str("cd missing");

    assert_eq!(input.suggestion_after_cursor(), None);
    assert!(!input.complete_or_accept(Some(&root)));
    assert_eq!(input.text(), "cd missing");
    assert!(input.history_previous());
    assert_eq!(input.text(), "cd missing-from-old-project");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn path_completion_escapes_shell_glob_characters() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-shell-escape-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("A Minecraft Movie (2025)")).unwrap();

    let candidates = completion_candidates("A", "cd ", false, true, Some(&root));
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.label == "A Minecraft Movie (2025)/")
        .expect("escaped directory completion");
    assert_eq!(candidate.replacement, "A\\ Minecraft\\ Movie\\ \\(2025\\)/");

    let escaped = completion_candidates(
        "A\\ Minecraft\\ Movie\\ \\",
        "cd ",
        false,
        true,
        Some(&root),
    );
    assert!(escaped
        .iter()
        .any(|candidate| candidate.replacement == "A\\ Minecraft\\ Movie\\ \\(2025\\)/"));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn known_non_path_commands_do_not_dump_cwd_argument_completions() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-flatpak-argument-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".hidden-dir")).unwrap();
    std::fs::create_dir_all(root.join("visible-dir")).unwrap();
    std::fs::write(root.join("local-file"), b"ok").unwrap();

    let candidates = completion_candidates("", "flatpak run ", false, false, Some(&root));
    assert!(candidates
        .iter()
        .any(|candidate| candidate.replacement == "--command="));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate.replacement == "visible-dir/"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate.replacement == ".hidden-dir/"));
    assert!(!candidates
        .iter()
        .any(|candidate| candidate.replacement == "local-file"));

    let explicit_path =
        completion_candidates("./", "flatpak run ", false, false, Some(&root));
    assert!(!explicit_path.is_empty());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn package_manager_completion_lists_package_json_scripts() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-package-json-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"scripts":{"build":"vite build","dev":"vite --host"}}"#,
    )
    .unwrap();

    let candidates = completion_candidates("b", "npm run ", false, false, Some(&root));

    let candidate = candidates
        .iter()
        .find(|candidate| candidate.replacement == "build ")
        .expect("package script completion");
    assert_eq!(candidate.label, "script build");
    assert_eq!(
        candidate.detail.as_deref(),
        Some("package.json: vite build")
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn git_subcommand_completion_appears_after_git() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("git sw");

    assert!(input.complete_or_accept(None));
    assert_eq!(input.text(), "git switch ");
    assert!(input.completion_menu_active());
}

#[test]
fn git_branch_completion_uses_local_and_remote_refs() {
    let root = std::env::temp_dir().join(format!(
        "neoism-completion-git-branch-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".git/refs/heads/feature")).unwrap();
    std::fs::create_dir_all(root.join(".git/refs/remotes/origin")).unwrap();
    std::fs::write(root.join(".git/refs/heads/feature/demo"), b"ref").unwrap();
    std::fs::write(root.join(".git/refs/remotes/origin/main"), b"ref").unwrap();
    std::fs::write(root.join(".git/refs/remotes/origin/HEAD"), b"ref").unwrap();

    let mut input = TerminalInputBuffer::default();
    input.insert_str("git switch f");
    assert!(input.complete_or_accept(Some(&root)));
    assert_eq!(input.text(), "git switch feature/demo");
    let local_candidates =
        completion_candidates("f", "git switch ", false, false, Some(&root));
    assert!(local_candidates.iter().any(|candidate| {
        candidate.label == "branch feature/demo"
            && candidate.detail.as_deref() == Some("Local git branch")
    }));

    let mut remote_input = TerminalInputBuffer::default();
    remote_input.insert_str("git switch o");
    assert!(remote_input.complete_or_accept(Some(&root)));
    assert_eq!(remote_input.text(), "git switch origin/main");
    assert!(remote_input
        .completion_items()
        .iter()
        .any(|item| item.contains("remote origin/main")));
    assert!(remote_input
        .completion_detail()
        .is_some_and(|detail| detail.contains("Remote git branch")));

    let _ = std::fs::remove_dir_all(&root);
}

fn snapshot(
    idx: usize,
    output_start: usize,
    status: BlockStatusKind,
) -> CommandBlockSnapshot {
    CommandBlockSnapshot {
        command: format!("cmd{idx}"),
        cwd: None,
        status,
        favorite: false,
        output_start_row: Some(output_start),
        duration_ms: 1.0,
    }
}

fn snapshot_with_command(
    command: &str,
    output_start: usize,
    status: BlockStatusKind,
) -> CommandBlockSnapshot {
    CommandBlockSnapshot {
        command: command.to_string(),
        cwd: None,
        status,
        favorite: false,
        output_start_row: Some(output_start),
        duration_ms: 1.0,
    }
}

fn row_with_char(ch: char, columns: usize) -> Row<Square> {
    let mut row = Row::<Square>::new(columns);
    row.inner[0] = Square::from_char(ch);
    row
}

fn row_with_text(text: &str, columns: usize) -> Row<Square> {
    let mut row = Row::<Square>::new(columns);
    for (idx, ch) in text.chars().take(columns).enumerate() {
        row.inner[idx] = Square::from_char(ch);
    }
    row
}

#[test]
fn compose_block_chrome_inserts_chrome_and_drops_echo_row() {
    // The PTY row whose abs == output_start_row is the shell's
    // command echo line; chrome paints the same command label
    // already, so compose must drop the echo so it doesn't show
    // up twice on screen. With single-row chrome the net change
    // is zero (drop 1 echo, add 1 chrome row), so the trailing
    // blank above the echo stays in the stream — chrome's META
    // band paints over it in pixel space, no row arithmetic.
    let snapshots = vec![snapshot(0, 100, BlockStatusKind::Ok)];
    let visible_rows = vec![
        row_with_char('p', 8),
        Row::<Square>::new(8),    // trailing blank of previous output
        row_with_text("cmd0", 8), // echo of "cmd0"
        row_with_char('a', 8),
        row_with_char('b', 8),
        row_with_char('c', 8),
    ];
    let visible_sources = (98..104).collect::<Vec<_>>();

    let frame =
        compose_block_chrome_window(visible_rows, visible_sources, &snapshots, 6, 98, 0)
            .frame;

    assert_eq!(frame.rows.len(), 6);
    assert_eq!(frame.block_header_spans.len(), 1);
    let span = frame.block_header_spans[0];
    // No drop-prev: the blank at abs 99 stays at index 1, chrome
    // takes the slot the echo used to occupy at index 2.
    assert_eq!(span.start_display_row, 2);
    assert_eq!(
        span.end_display_row,
        (2 + COMMAND_BLOCK_CHROME_ROWS) as isize
    );
    assert_eq!(frame.source_row_indices[2], None);
    // Output continues right after the chrome at abs 101.
    assert_eq!(
        frame.source_row_indices[2 + COMMAND_BLOCK_CHROME_ROWS],
        Some(101)
    );
    assert!(!frame.source_row_indices.iter().any(|s| *s == Some(100)));
    // The blank at abs 99 IS in the frame (no drop-prev).
    assert_eq!(frame.source_row_indices[1], Some(99));
}

#[test]
fn compose_window_reports_text_matched_echo_rows_for_scroll_cursor() {
    // Repeated commands are common (`ls` ten times while testing).
    // After resize/reflow, stored output_start_row values can be
    // stale, so compose falls back to matching the command text in
    // visual order. The scroll cursor must consume the same resolved
    // echo rows, or it will think these rows are height=1 while the
    // renderer paints them as height=2 and some blocks become
    // unreachable.
    let snapshots = (0..4)
        .map(|idx| snapshot_with_command("ls", 1000 + idx, BlockStatusKind::Ok))
        .collect::<Vec<_>>();
    let visible_rows = vec![
        row_with_text("ls", 8),
        row_with_char('a', 8),
        row_with_text("ls", 8),
        row_with_char('b', 8),
        row_with_text("ls", 8),
        row_with_char('c', 8),
        row_with_text("ls", 8),
        row_with_char('d', 8),
    ];
    let visible_sources = (10..18).collect::<Vec<_>>();

    let window =
        compose_block_chrome_window(visible_rows, visible_sources, &snapshots, 12, 10, 0);

    assert_eq!(
        window.echo_rows,
        BTreeSet::from([10usize, 12usize, 14usize, 16usize])
    );
    assert_eq!(window.frame.block_header_spans.len(), 4);
}

#[test]
fn text_matched_echo_uses_nearest_stale_output_row_after_reflow() {
    // A width change can move the visual echo row far away from
    // the stored output_start_row. The matcher must not skip all
    // blocks just because the visible window starts after every
    // stale abs value; repeated commands should still bind to the
    // nearest snapshot so the chrome formatting survives reflow.
    let snapshots = vec![
        snapshot_with_command("ls", 12, BlockStatusKind::Ok),
        snapshot_with_command("ls", 23, BlockStatusKind::Ok),
        snapshot_with_command("ls", 34, BlockStatusKind::Ok),
    ];
    let visible_rows = vec![row_with_text("ls", 8), row_with_char('a', 8)];
    let visible_sources = vec![91, 92];

    let window =
        compose_block_chrome_window(visible_rows, visible_sources, &snapshots, 4, 91, 0);

    assert_eq!(window.echo_rows, BTreeSet::from([91usize]));
    assert_eq!(window.frame.block_header_spans[0].block_idx, 2);
}

#[test]
fn stale_abs_match_does_not_steal_reflowed_echo_row() {
    let snapshots = vec![snapshot_with_command("ls", 100, BlockStatusKind::Ok)];
    let visible_rows = vec![
        row_with_text("output", 8),
        row_with_text("ls", 8),
        row_with_char('a', 8),
    ];
    let visible_sources = vec![100, 101, 102];

    let window =
        compose_block_chrome_window(visible_rows, visible_sources, &snapshots, 5, 100, 0);

    assert_eq!(window.echo_rows, BTreeSet::from([101usize]));
    assert_eq!(window.frame.block_header_spans[0].start_display_row, 1);
}

#[test]
fn text_matched_echo_preserves_repeated_command_order_after_reflow() {
    let snapshots = vec![
        snapshot_with_command("ls", 12, BlockStatusKind::Ok),
        snapshot_with_command("ls", 23, BlockStatusKind::Ok),
        snapshot_with_command("ls", 34, BlockStatusKind::Ok),
    ];
    let visible_rows = vec![
        row_with_text("ls", 8),
        row_with_char('a', 8),
        row_with_text("ls", 8),
        row_with_char('b', 8),
        row_with_text("ls", 8),
        row_with_char('c', 8),
    ];
    let visible_sources = vec![91, 92, 102, 103, 113, 114];

    let window =
        compose_block_chrome_window(visible_rows, visible_sources, &snapshots, 10, 91, 0);

    let block_indices = window
        .frame
        .block_header_spans
        .iter()
        .map(|span| span.block_idx)
        .collect::<Vec<_>>();
    assert_eq!(block_indices, vec![0, 1, 2]);
}

#[test]
fn compose_block_chrome_skips_blocks_with_header_above_viewport() {
    // Warp does NOT pin chrome — the header for a block whose
    // output_start_row is above visible_sources[0] stays scrolled
    // out of view.
    let snapshots = vec![snapshot(0, 50, BlockStatusKind::Ok)];
    let visible_rows = vec![
        row_with_char('a', 8),
        row_with_char('b', 8),
        row_with_char('c', 8),
        row_with_char('d', 8),
    ];
    let visible_sources = (100..104).collect::<Vec<_>>();

    let frame = compose_block_chrome(visible_rows, visible_sources, &snapshots, 4);

    assert!(frame.block_header_spans.is_empty());
}

#[test]
fn compose_block_chrome_bottom_aligns_when_pty_is_sparse() {
    // Sparse output: PTY has 2 rows of content plus 4 blank rows.
    // Bottom-align should move trailing blanks to the head so
    // content sits just above where the composer renders.
    let mut visible_rows = Vec::with_capacity(6);
    for ch in [Some('a'), Some('b'), None, None, None, None] {
        let mut row = Row::<Square>::new(8);
        if let Some(ch) = ch {
            row.inner[0] = Square::from_char(ch);
        }
        visible_rows.push(row);
    }
    let visible_sources = (0..6).collect::<Vec<_>>();

    let frame = compose_block_chrome(visible_rows, visible_sources, &[], 6);

    assert_eq!(frame.rows.len(), 6);
    // First four rows are bottom-align padding (None sources).
    for row in 0..4 {
        assert_eq!(frame.source_row_indices[row], None);
    }
    // Last two rows still carry the original content.
    assert_eq!(frame.source_row_indices[4], Some(0));
    assert_eq!(frame.source_row_indices[5], Some(1));
}

fn shell_state(awaiting: bool, running: bool) -> ShellPromptState {
    ShellPromptState {
        awaiting_command: awaiting,
        running_command: running,
        last_exit_code: None,
    }
}

#[test]
fn fresh_terminal_captures_first_command_before_prompt_arrives() {
    // A brand-new pane: the shell hasn't printed its first OSC 133
    // prompt yet, so `awaiting_command` is still false. The composer
    // must still own the empty command line, otherwise the first
    // keystrokes leak to the raw PTY and the first command runs
    // wrong / "loses" its leading characters.
    let input = TerminalInputBuffer::default();
    assert!(input.should_capture_input(shell_state(false, false), false));
}

#[test]
fn boot_window_yields_once_a_command_is_running() {
    // If a foreground command is already running on a fresh attach
    // (e.g. reconnecting to a live session) the keystrokes belong to
    // that process, not the composer.
    let input = TerminalInputBuffer::default();
    assert!(!input.should_capture_input(shell_state(false, true), false));
}

#[test]
fn composer_never_relinquishes_a_pending_command_mid_edit() {
    // Once shell integration has reported a prompt, the boot window
    // is over. A later transient `awaiting_command == false` (e.g.
    // the shell repainting between commands) must NOT hand a
    // half-typed command back to the raw PTY: that is exactly the
    // "hides characters" divergence. Pending text pins ownership.
    let mut input = TerminalInputBuffer::default();
    input.sync_shell_state(shell_state(true, false));
    input.insert_str("git stat");

    // Awaiting flips false but the user is mid-command.
    assert!(input.should_capture_input(shell_state(false, false), false));
    // Even while a command is "running", a pending edit stays ours.
    assert!(input.should_capture_input(shell_state(false, true), false));
}

#[test]
fn empty_line_after_first_prompt_follows_awaiting_command() {
    // Steady state: with no pending text and the first prompt seen,
    // the gate tracks `awaiting_command` exactly as before so we
    // don't steal keys from foreground commands on non-integrated
    // shells.
    let mut input = TerminalInputBuffer::default();
    input.sync_shell_state(shell_state(true, false));
    // Prompt finished, command now running, composer empty.
    input.sync_shell_state(shell_state(false, true));
    assert!(!input.should_capture_input(shell_state(false, true), false));
    // Back at a prompt with an empty line — composer owns it again.
    assert!(input.should_capture_input(shell_state(true, false), false));
}

#[test]
fn alt_screen_and_passthrough_never_capture() {
    let mut input = TerminalInputBuffer::default();
    input.insert_str("anything");
    // Alt-screen TUI owns the grid even with pending text.
    assert!(!input.should_capture_input(shell_state(true, false), true));
    // Passthrough session (sh/ssh) bypasses our shell hooks.
    input.set_passthrough_session_active(true);
    assert!(!input.should_capture_input(shell_state(true, false), false));
}

#[test]
fn submitted_command_matches_displayed_buffer_exactly() {
    // The displayed buffer and the bytes sent on Enter share one
    // source: `submit_with_context` returns the same string the
    // composer rendered, which `command_payload` then frames.
    let mut input = TerminalInputBuffer::default();
    input.insert_str("echo hello world");
    let displayed = input.text().to_string();
    let submitted = input.submit_with_context(None, None);
    assert_eq!(submitted, displayed);
    assert_eq!(
        TerminalShellKind::Bash.command_payload(&submitted, true),
        b"echo hello world\n"
    );
}
