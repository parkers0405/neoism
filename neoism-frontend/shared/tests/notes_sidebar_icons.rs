use std::path::PathBuf;

use neoism_ui::panels::file_tree::icons::icon_for_file;
use neoism_ui::panels::notes_sidebar::{NotesSidebar, NOTES_ICONS_FILE};

fn scratch_vault(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("neoism-{name}-{}", std::process::id()))
}

#[test]
fn markdown_uses_the_paper_with_lines_default_icon() {
    assert_eq!(icon_for_file("note.md").0, "\u{f15c}");
    assert_eq!(icon_for_file("README.md").0, "\u{f15c}");
}

#[test]
fn saved_default_override_does_not_mask_root_note_frontmatter_icon() {
    let root = scratch_vault("root-note-icon");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let note = root.join("TASKS.md");
    std::fs::write(&note, "---\nicon: \u{1f4a1}\n---\n# TASKS\n").unwrap();
    std::fs::write(root.join(NOTES_ICONS_FILE), "{\"TASKS.md\":\"\u{f15c}\"}").unwrap();

    let mut sidebar = NotesSidebar::default();
    sidebar.set_workspace("Test", Some(root.clone()));
    assert_eq!(
        sidebar.note_icon_for_path(&note).as_deref(),
        Some("\u{1f4a1}")
    );

    let _ = std::fs::remove_dir_all(root);
}
