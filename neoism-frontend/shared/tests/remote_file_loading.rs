use neoism_ui::editor::{code::CodePane, markdown::MarkdownPane};
use neoism_ui::services::FileOpenSource;

#[test]
fn same_path_different_source_requires_explicit_recovery_not_migration() {
    assert!(FileOpenSource::LocalOnly.conflicts_with(true, false));
    assert!(FileOpenSource::Host.conflicts_with(false, true));
    assert!(!FileOpenSource::LocalOnly.conflicts_with(false, true));
    assert!(!FileOpenSource::Host.conflicts_with(true, false));
}

#[test]
fn explicitly_local_vault_while_joined_loads_and_saves_locally() {
    let temp = tempfile::tempdir().unwrap();
    let code_path = temp.path().join("文 件.rs");
    let note_path = temp.path().join("Local note.md");
    std::fs::write(&code_path, "guest code").unwrap();
    std::fs::write(&note_path, "# Guest note").unwrap();
    // The exact origin policy used by open_path_from_notes_sidebar, followed
    // by the same constructors used by the desktop Context factories.
    let mut sidebar = neoism_ui::panels::notes_sidebar::NotesSidebar::default();
    // Deliberately select a local vault with the SAME pathname as a previously
    // viewed remote vault. Origin must not be inferred from string equality.
    sidebar.set_remote_workspace("host", Some(temp.path().into()));
    assert!(sidebar.is_remote_workspace());
    sidebar.set_workspace("local", Some(temp.path().into()));
    assert!(!sidebar.is_remote_workspace());
    let source = FileOpenSource::notes(true, sidebar.is_remote_workspace());
    assert_eq!(source, FileOpenSource::LocalOnly);
    let mut code = CodePane::load_with_source(code_path.clone(), source);
    let mut note = MarkdownPane::load_with_source(note_path.clone(), source);
    assert_eq!(code.buffer.lines[0], "guest code");
    assert_eq!(note.lines[0], "# Guest note");
    assert!(!code.remote_source && !note.remote_source);
    assert!(code.local_only && note.local_only);
    // Production CRDT OpenBuffer and SaveBuffer gates both use this method.
    assert!(!code.workspace_sync_ready() && !note.workspace_sync_ready());
    code.buffer.lines[0] = "local code edits".into();
    note.lines[0] = "# Local note edits".into();
    // A late remote reply or a generic remote-load request cannot rehome a
    // deliberately local document or discard its unsaved edits.
    code.mark_remote_loading();
    note.mark_remote_loading();
    code.apply_remote_source("wrong host bytes");
    note.apply_remote_source("wrong host bytes");
    code.save().unwrap();
    note.save().unwrap();
    assert_eq!(
        std::fs::read_to_string(&code_path).unwrap(),
        "local code edits"
    );
    assert_eq!(
        std::fs::read_to_string(&note_path).unwrap(),
        "# Local note edits"
    );
    assert!(!code.workspace_sync_ready() && !note.workspace_sync_ready());
    assert_eq!(FileOpenSource::notes(false, false), FileOpenSource::Local);
    sidebar.set_remote_workspace("host", Some(temp.path().into()));
    assert!(sidebar.is_remote_workspace());
    assert!(!sidebar.contains_path(&note_path), "a source switch must not display guest entries as host entries");
}

#[test]
fn joined_host_source_does_not_read_or_save_coincident_guest_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("same-name.md");
    std::fs::write(&path, "guest sentinel").unwrap();
    let source = FileOpenSource::notes(true, true);
    assert_eq!(source, FileOpenSource::Host);
    let mut code = CodePane::load_with_source(path.clone(), source);
    let mut note = MarkdownPane::load_with_source(path.clone(), source);
    assert_eq!(code.buffer.lines, vec![String::new()]);
    assert_eq!(note.lines, vec![String::new()]);
    code.mark_remote_loading();
    note.mark_remote_loading();
    code.buffer.lines[0] = "unsynced code".into();
    note.lines[0] = "unsynced note".into();
    code.fail_remote_loading("host unavailable");
    note.fail_remote_loading("host unavailable");
    assert!(!code.remote_content_pending && !note.remote_content_pending);
    assert!(!code.workspace_sync_ready() && !note.workspace_sync_ready());
    assert!(code.save().is_err() && note.save().is_err());
    assert_eq!(code.buffer.lines[0], "unsynced code");
    assert_eq!(note.lines[0], "unsynced note");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "guest sentinel");
}

#[test]
fn local_new_file_and_new_backing_path_save_still_create_files() {
    let temp = tempfile::tempdir().unwrap();
    let code_path = temp.path().join("new.rs");
    let note_path = temp.path().join("new.md");
    let mut code = CodePane::load_with_source(code_path.clone(), FileOpenSource::Local);
    let mut note =
        MarkdownPane::load_with_source(note_path.clone(), FileOpenSource::Local);
    // Missing local loads are not CRDT-bound; the first save uses local I/O.
    assert!(!code.workspace_sync_ready() && !note.workspace_sync_ready());
    code.buffer.lines = vec!["new code".into()];
    note.lines = vec!["# New note".into()];
    code.save().unwrap();
    note.save().unwrap();
    assert!(code.workspace_sync_ready() && note.workspace_sync_ready());
    assert_eq!(std::fs::read_to_string(code_path).unwrap(), "new code");
    assert_eq!(std::fs::read_to_string(note_path).unwrap(), "# New note");
    // The local save API remains create-capable if a caller supplies a new
    // backing path (no native code/Markdown Save As command exists today).
    code.path = temp.path().join("copy.rs");
    note.path = temp.path().join("copy.md");
    code.save().unwrap();
    note.save().unwrap();
    assert!(code.path.exists() && note.path.exists());
}

#[test]
fn failed_code_read_clears_pending_retains_edits_and_cannot_write_guest_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("main.rs");
    std::fs::write(&path, "guest sentinel").unwrap();
    let mut pane = CodePane::new(path.clone(), "original");
    pane.buffer.lines[0] = "unsaved recovery text".into();
    pane.mark_remote_loading();
    pane.fail_remote_loading("not found");
    assert!(!pane.remote_content_pending);
    assert!(pane.error.as_ref().unwrap().contains("not found"));
    assert_eq!(pane.buffer.lines[0], "unsaved recovery text");
    assert!(pane.save().is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "guest sentinel");
}

#[test]
fn failed_markdown_read_clears_timer_and_cannot_create_empty_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("README.md");
    let mut pane = MarkdownPane::from_source(path.clone(), "");
    pane.mark_remote_loading();
    pane.fail_remote_loading("permission denied");
    assert!(!pane.remote_content_pending);
    assert!(pane.error.as_ref().unwrap().contains("permission denied"));
    assert!(pane.save().is_err());
    assert!(!path.exists());
    // Reopening is retryable. A successful read may now bind the real CRDT
    // document, but local save must remain prohibited after loading too.
    pane.mark_remote_loading();
    pane.apply_remote_source("# Host note");
    assert!(pane.error.is_none());
    assert!(!pane.remote_content_pending);
    assert_eq!(pane.lines[0], "# Host note");
    assert!(pane.save().is_err());
    assert!(!path.exists());
}

#[test]
fn delayed_success_does_not_clobber_edits_made_during_read() {
    let mut code = CodePane::new("/host/main.rs".into(), "");
    code.mark_remote_loading();
    code.buffer.lines[0] = "local edit".into();
    code.apply_remote_source("host source");
    assert_eq!(code.buffer.lines[0], "local edit");
    assert!(!code.remote_content_pending);
    assert!(code.error.is_some());

    let mut markdown = MarkdownPane::from_source("/host/README.md".into(), "");
    markdown.mark_remote_loading();
    markdown.lines[0] = "local edit".into();
    markdown.apply_remote_source("host source");
    assert_eq!(markdown.lines[0], "local edit");
    assert!(!markdown.remote_content_pending);
    assert!(markdown.error.is_some());
}

#[test]
fn remote_notes_never_read_matching_guest_vault_or_icon_map() {
    let temp = tempfile::tempdir().unwrap();
    let note = temp.path().join("README.md");
    std::fs::write(&note, "---\nicon: GUEST\n---\n# Wrong machine").unwrap();
    std::fs::write(
        temp.path().join(".neoism-icons.json"),
        r#"{"README.md":"GUEST"}"#,
    )
    .unwrap();
    let mut sidebar = neoism_ui::panels::notes_sidebar::NotesSidebar::default();
    sidebar.set_remote_workspace("host", Some(temp.path().into()));
    assert!(
        !sidebar.contains_path(&note),
        "remote setup must not walk guest files"
    );
    sidebar.set_remote_entries_from_host(vec![(note.clone(), false)]);
    assert!(sidebar.contains_path(&note));
    assert!(sidebar.note_icon_for_path(&note).is_none());
    sidebar.refresh_notes();
    assert!(sidebar.note_icon_for_path(&note).is_none());
    sidebar.set_workspace("local", Some(temp.path().into()));
    assert_eq!(sidebar.note_icon_for_path(&note).as_deref(), Some("GUEST"));
}

#[test]
fn unix_backslash_and_directory_paths_do_not_alias_in_tabs() {
    let mut tabs = neoism_ui::panels::buffer_tabs::BufferTabs::<()>::new();
    let literal = std::path::PathBuf::from(r"/host/src\main.rs");
    let nested = std::path::PathBuf::from("/host/src/main.rs");
    tabs.open_path(literal.clone());
    tabs.open_path(nested.clone());
    assert_ne!(tabs.find_path(&literal), tabs.find_path(&nested));
}
