
use super::*;

#[test]
fn generated_task_update_uses_source_file_link() {
    let update = parse_generated_task_update(
        "- [x] Ship it [[Other]] [[/tmp/neoism-note.md-42|note.md:42]]",
    )
    .unwrap();

    assert_eq!(update.path, PathBuf::from("/tmp/neoism-note.md"));
    assert_eq!(update.line, 42);
    assert!(update.checked);
}

#[test]
fn generated_task_update_uses_hidden_source_marker() {
    let marker = generated_task_source_marker(Path::new("/tmp/neoism note.md"), 42);
    let update = parse_generated_task_update(&format!("- [ ] Ship it {marker}")).unwrap();

    assert_eq!(update.path, PathBuf::from("/tmp/neoism note.md"));
    assert_eq!(update.line, 42);
    assert!(!update.checked);
}

#[test]
fn generated_task_save_toggles_source_checkbox_only() {
    let mut line = "  - [ ] Ship it #neoism".to_string();
    assert!(set_task_line_checked(&mut line, true));
    assert_eq!(line, "  - [x] Ship it #neoism");
    assert!(!set_task_line_checked(&mut line, true));
    assert!(set_task_line_checked(&mut line, false));
    assert_eq!(line, "  - [ ] Ship it #neoism");
}
