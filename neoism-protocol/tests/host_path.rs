use neoism_protocol::host_path::HostPath;

#[test]
fn filesystem_roots_round_trip_relative_wire_paths() {
    for (root, expected) in [
        ("/", "/src/文 件.rs"),
        (r"C:\", r"C:\src\文 件.rs"),
        ("C:/", "C:/src\\文 件.rs"),
        (r"\\Server\Share\", r"\\Server\Share\src\文 件.rs"),
    ] {
        let root = HostPath::new(root);
        assert_eq!(root.relative(root.as_str()), Some(String::new()));
        assert_eq!(root.join("").as_str(), root.as_str());
        let file = root.join("src/文 件.rs");
        assert_eq!(file.as_str(), expected);
        assert_eq!(
            root.relative(file.as_str()).as_deref(),
            Some("src/文 件.rs")
        );
    }
}

#[test]
fn relative_paths_require_a_component_boundary() {
    for (root, sibling, child) in [
        ("/work", "/workspace/file.rs", "/work/file.rs"),
        ("/work/", "/workspace/file.rs", "/work/file.rs"),
        (r"C:\Work", r"C:\Workspace\file.rs", r"C:\Work\file.rs"),
        (
            r"\\Server\Share",
            r"\\Server\Shared\file.rs",
            r"\\Server\Share\file.rs",
        ),
    ] {
        let root = HostPath::new(root);
        assert_eq!(root.relative(sibling), None);
        assert_eq!(root.relative(child).as_deref(), Some("file.rs"));
    }
}

#[test]
fn windows_relative_paths_accept_mixed_separators_but_preserve_case() {
    let root = HostPath::new(r"C:\Work");
    assert_eq!(
        root.relative(r"C:/Work/src\Main.rs").as_deref(),
        Some("src/Main.rs")
    );
    assert_eq!(root.relative(r"c:\Work\src\Main.rs"), None);
    assert_eq!(root.relative(r"C:\work\src\Main.rs"), None);
    // Drive-relative paths and Unix double-slash roots are not Windows roots.
    assert!(!HostPath::new("C:Work").is_windows());
    assert!(!HostPath::new("//server/share").is_windows());
}

#[test]
fn document_ids_preserve_literal_uri_characters_and_dot_segments() {
    let root = HostPath::new("/work");
    let relative = "./notes/../100% #draft?.md";
    let file = root.join(relative);
    assert_eq!(root.relative(file.as_str()).as_deref(), Some(relative));
    assert_eq!(file.buffer_id(), "file:///work/./notes/../100% #draft?.md");
}
