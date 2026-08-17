//! Integration tests for the lifted markdown editor.
//!
//! The bulk of the state machine's behavioural coverage lives in
//! `neoism-ui/src/editor/markdown/tests.rs` (the original suite, moved
//! verbatim from `frontends/neoism/src/editor/markdown/state/tests.rs`).
//! These integration tests exercise the public surface a host shell
//! actually drives — load, insert, save, mode switch, helper lookups —
//! so a regression in the cross-crate API will fail outside the
//! private module's `super::*` reach.

use std::path::PathBuf;

use neoism_ui::editor::markdown::{
    is_markdown_path, is_misspelled_word, parse_markdown_link_inner,
    parse_table_cell_bounds, spelling_suggestions, MarkdownMode, MarkdownPane,
};

fn temp_md_path(label: &str) -> PathBuf {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    std::env::temp_dir().join(format!("neoism-md-{label}-{unique}.md"))
}

#[test]
fn markdown_state_loads_blank_pane_for_missing_path() {
    let path = temp_md_path("missing");
    let pane = MarkdownPane::load(path.clone());
    assert_eq!(pane.path, path);
    assert_eq!(pane.lines, vec![String::new()]);
    assert_eq!(pane.mode, MarkdownMode::Normal);
    assert_eq!(pane.cursor_line, 0);
    assert_eq!(pane.cursor_col, 0);
    assert!(!pane.is_dirty());
}

#[test]
fn markdown_state_inserts_text_and_marks_dirty() {
    let mut pane = MarkdownPane::load(temp_md_path("insert"));
    pane.enter_insert();
    pane.insert_text("# Heading");
    assert_eq!(pane.lines, vec!["# Heading".to_string()]);
    assert_eq!(pane.cursor_col, "# Heading".len());
    assert!(pane.is_dirty());
}

#[test]
fn markdown_state_handles_code_block_insertion() {
    let mut pane = MarkdownPane::load(temp_md_path("code"));
    pane.enter_insert();
    pane.insert_text("```rust");
    pane.insert_newline();
    pane.insert_text("fn main() {}");
    pane.insert_newline();
    pane.insert_text("```");
    assert_eq!(
        pane.lines,
        vec![
            "```rust".to_string(),
            "fn main() {}".to_string(),
            "```".to_string(),
        ]
    );
}

#[test]
fn markdown_state_mode_round_trips_through_visual() {
    let mut pane = MarkdownPane::load(temp_md_path("modes"));
    assert_eq!(pane.mode, MarkdownMode::Normal);
    pane.enter_insert();
    assert_eq!(pane.mode, MarkdownMode::Insert);
    pane.enter_normal();
    assert_eq!(pane.mode, MarkdownMode::Normal);
    pane.enter_visual();
    assert_eq!(pane.mode, MarkdownMode::Visual);
    pane.enter_normal();
    assert_eq!(pane.mode, MarkdownMode::Normal);
}

#[test]
fn is_markdown_path_matches_common_extensions() {
    assert!(is_markdown_path(std::path::Path::new("notes/a.md")));
    assert!(is_markdown_path(std::path::Path::new("README.markdown")));
    assert!(!is_markdown_path(std::path::Path::new("main.rs")));
    assert!(!is_markdown_path(std::path::Path::new("plain.txt")));
}

#[test]
fn parse_markdown_link_inner_extracts_path_and_line() {
    let (target, line) =
        parse_markdown_link_inner("@notes/page.md-12").expect("wiki link parse");
    assert_eq!(target, "notes/page.md");
    assert_eq!(line, Some(12));

    let (target, line) =
        parse_markdown_link_inner("@plain.md").expect("plain link parse");
    assert_eq!(target, "plain.md");
    assert_eq!(line, None);

    // Without the `@` sentinel the helper rejects the inner.
    assert!(parse_markdown_link_inner("plain.md").is_none());
}

#[test]
fn parse_table_cell_bounds_splits_pipe_separated_row() {
    let bounds = parse_table_cell_bounds("| foo | bar |").expect("table cell parse");
    assert_eq!(bounds.len(), 2);
    assert_eq!(bounds[0].content_start, 2);
    assert_eq!(bounds[0].content_end, 5);
    assert_eq!(bounds[1].content_start, 8);
    assert_eq!(bounds[1].content_end, 11);
}

#[test]
fn spellcheck_primitives_do_not_panic_without_dictionary() {
    // On hosts without /usr/share/dict the dictionary load returns
    // None, and these helpers must short-circuit to "ok" / empty.
    let _ = is_misspelled_word("typoooo");
    let suggestions = spelling_suggestions("typoooo");
    assert!(suggestions.len() <= 5);
}

#[test]
fn spelling_replacement_rejects_a_stale_diagnostic_range() {
    let path = temp_md_path("stale-spelling-action");
    let mut pane = MarkdownPane::load(path.clone());
    pane.enter_insert();
    pane.insert_text("A mispalled word");

    assert!(!pane.replace_spelling_word(0, 2, 11, "different", "misspelled"));
    assert_eq!(pane.lines[0], "A mispalled word");
    assert!(pane.replace_spelling_word(0, 2, 11, "mispalled", "misspelled"));
    assert_eq!(pane.lines[0], "A misspelled word");
    let _ = std::fs::remove_file(path);
}

#[test]
fn markdown_state_saves_and_round_trips_through_disk() {
    let path = temp_md_path("save");
    let mut pane = MarkdownPane::load(path.clone());
    pane.enter_insert();
    pane.insert_text("hello world");
    pane.save().expect("save should succeed");
    assert!(!pane.is_dirty());

    let reloaded = MarkdownPane::load(path.clone());
    assert_eq!(reloaded.lines, vec!["hello world".to_string()]);
    let _ = std::fs::remove_file(&path);
}
