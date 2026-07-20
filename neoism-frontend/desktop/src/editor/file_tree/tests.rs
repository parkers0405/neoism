use std::collections::HashSet;
use std::path::PathBuf;

use super::git::parse_git_status;
use super::icons::icon_for;
use super::scan::{normalize_path, scan_dir, scan_dir_with_open};
use super::state::FileTree;
use super::types::{GitStatus, NodeKind, TreeEntry, VirtualEntryKind};
use super::virtuals::NEOISM_FOLDER_ICON_COLOR;
use super::{FILE_TREE_WIDTH, FOLDER_ICON_COLOR, FRAME_STROKE, ROW_HEIGHT};

fn sample_entries(n: usize) -> Vec<TreeEntry> {
    (0..n)
        .map(|i| TreeEntry {
            label: format!("file_{i}.rs"),
            depth: 0,
            kind: NodeKind::File,
            path: None,
            git_status: GitStatus::None,
            virtual_kind: None,
        })
        .collect()
}

#[test]
fn select_next_clamps_at_end() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(3));
    t.select_next();
    t.select_next();
    t.select_next();
    t.select_next();
    assert_eq!(t.selected_index(), 2);
}

#[test]
fn select_prev_clamps_at_zero() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(3));
    t.select_prev();
    assert_eq!(t.selected_index(), 0);
}

#[test]
fn set_entries_clamps_selection() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(5));
    for _ in 0..4 {
        t.select_next();
    }
    assert_eq!(t.selected_index(), 4);
    // Shrink → selection has to fold back onto the new last row.
    t.set_entries(sample_entries(2));
    assert_eq!(t.selected_index(), 1);
}

#[test]
fn hit_test_returns_none_when_hidden() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(3));
    assert_eq!(t.hit_test(10.0, 10.0, 0.0, 200.0), None);
}

#[test]
fn hit_test_maps_y_to_row() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(10));
    t.set_visible(true);
    let y_top = 30.0;
    // First row: middle of row 0.
    let mid_row_0 = y_top + ROW_HEIGHT / 2.0;
    assert_eq!(t.hit_test(20.0, mid_row_0, y_top, 400.0), Some(0));
    // Third row.
    let mid_row_2 = y_top + ROW_HEIGHT * 2.0 + ROW_HEIGHT / 2.0;
    assert_eq!(t.hit_test(20.0, mid_row_2, y_top, 400.0), Some(2));
}

#[test]
fn hit_test_rejects_outside_panel() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(10));
    t.set_visible(true);
    // Past panel right edge.
    assert_eq!(t.hit_test(FILE_TREE_WIDTH + 5.0, 40.0, 0.0, 400.0), None);
    // Above panel top.
    assert_eq!(t.hit_test(20.0, 10.0, 30.0, 400.0), None);
}

#[test]
fn toggle_dir_collapse_drops_descendants() {
    // Hand-crafted entries: dir A (open) with two children, then
    // sibling file B. Collapsing A should strip its two children
    // but leave A and B intact.
    let mut t = FileTree::new();
    t.set_entries(vec![
        TreeEntry {
            label: "A".into(),
            depth: 0,
            kind: NodeKind::Dir { open: true },
            path: Some(PathBuf::from("/tmp/A")),
            git_status: GitStatus::None,
            virtual_kind: None,
        },
        TreeEntry {
            label: "child1".into(),
            depth: 1,
            kind: NodeKind::File,
            path: Some(PathBuf::from("/tmp/A/child1")),
            git_status: GitStatus::None,
            virtual_kind: None,
        },
        TreeEntry {
            label: "child2".into(),
            depth: 1,
            kind: NodeKind::File,
            path: Some(PathBuf::from("/tmp/A/child2")),
            git_status: GitStatus::None,
            virtual_kind: None,
        },
        TreeEntry {
            label: "B".into(),
            depth: 0,
            kind: NodeKind::File,
            path: Some(PathBuf::from("/tmp/B")),
            git_status: GitStatus::None,
            virtual_kind: None,
        },
    ]);
    t.toggle_dir_at(0);
    assert_eq!(t.entries().len(), 2);
    assert_eq!(t.entries()[0].label, "A");
    assert_eq!(t.entries()[1].label, "B");
    assert_eq!(t.entries()[0].kind, NodeKind::Dir { open: false });
}

#[test]
fn toggle_on_file_is_noop() {
    let mut t = FileTree::new();
    t.set_entries(vec![TreeEntry {
        label: "f.rs".into(),
        depth: 0,
        kind: NodeKind::File,
        path: Some(PathBuf::from("/tmp/f.rs")),
        git_status: GitStatus::None,
        virtual_kind: None,
    }]);
    let result = t.toggle_dir_at(0);
    assert!(result.is_none());
    assert_eq!(t.entries().len(), 1);
}

#[test]
fn reveal_directory_expands_selects_and_flashes_target() {
    let root =
        std::env::temp_dir().join(format!("neoism-tree-reveal-{}", std::process::id()));
    let target = root.join("src").join("bin");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("main.rs"), "").unwrap();

    let mut t = FileTree::new();
    t.populate_from_dir(&root);
    let ix = t.reveal_directory(&target).unwrap();

    assert_eq!(t.selected_index(), ix);
    assert_eq!(t.entries()[ix].path.as_deref(), Some(target.as_path()));
    assert_eq!(t.entries()[ix].kind, NodeKind::Dir { open: true });

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn hit_test_returns_none_past_last_row() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(2));
    t.set_visible(true);
    let y_far = ROW_HEIGHT * 5.0;
    assert_eq!(t.hit_test(20.0, y_far, 0.0, 400.0), None);
}

#[test]
fn tree_scroll_does_not_reserve_virtual_footer_rows() {
    let mut t = FileTree::new();
    t.set_entries(sample_entries(20));

    assert_eq!(
        t.visible_rows_for_panel_height(ROW_HEIGHT * 5.0 + FRAME_STROKE * 2.0),
        5
    );
}

#[test]
fn icon_for_routes_extensions() {
    let make = |label: &str| TreeEntry {
        label: label.into(),
        depth: 0,
        kind: NodeKind::File,
        path: None,
        git_status: GitStatus::None,
        virtual_kind: None,
    };
    // Smoke-test a handful of distinct extensions resolve to
    // distinct glyph/color pairs.
    let rs = icon_for(&make("lib.rs"));
    let py = icon_for(&make("main.py"));
    let dock = icon_for(&make("Dockerfile"));
    let gd = icon_for(&make("player.gd"));
    let shader = icon_for(&make("water.gdshader"));
    let shader_include = icon_for(&make("common.gdshaderinc"));
    let tscn = icon_for(&make("world.tscn"));
    let tres = icon_for(&make("material.tres"));
    let gd_uid = icon_for(&make("player.gd.uid"));
    let project = icon_for(&make("project.godot"));
    let unknown = icon_for(&make("weird.xyz"));
    assert_ne!(rs.0, py.0);
    assert_ne!(rs.1, py.1);
    assert_eq!(dock.0, "\u{f308}");
    assert_eq!(gd.0, "\u{e65f}");
    assert_eq!(shader, gd);
    assert_eq!(shader_include, gd);
    assert_eq!(tscn, gd);
    assert_eq!(tres, gd);
    assert_eq!(gd_uid, gd);
    assert_eq!(project, gd);
    assert_eq!(unknown.0, "\u{f15b}");
}

#[test]
fn folder_icon_color_is_blue() {
    let entry = TreeEntry {
        label: "src".into(),
        depth: 0,
        kind: NodeKind::Dir { open: false },
        path: None,
        git_status: GitStatus::None,
        virtual_kind: None,
    };
    let (_, color) = icon_for(&entry);
    assert_eq!(color, FOLDER_ICON_COLOR);
}

#[test]
fn neoism_workspace_folder_icon_is_dark_blue() {
    let entry = TreeEntry {
        label: "Neoism".into(),
        depth: 0,
        kind: NodeKind::Dir { open: false },
        path: None,
        git_status: GitStatus::None,
        virtual_kind: Some(VirtualEntryKind::NeoismWorkspace),
    };
    let (_, color) = icon_for(&entry);
    assert_eq!(color, NEOISM_FOLDER_ICON_COLOR);
}

#[test]
fn populate_same_root_preserves_open_folders() {
    let root = std::env::temp_dir()
        .join(format!("neoism-tree-preserve-open-{}", std::process::id()));
    let src = root.join("src");
    let nested = src.join("nested");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(src.join("lib.rs"), "").unwrap();
    std::fs::write(nested.join("mod.rs"), "").unwrap();

    let mut tree = FileTree::new();
    tree.populate_from_dir(&root);
    let src_ix = tree
        .entries()
        .iter()
        .position(|entry| entry.path.as_deref() == Some(src.as_path()))
        .unwrap();
    tree.toggle_dir_at(src_ix);
    let selected_ix = tree
        .entries()
        .iter()
        .position(|entry| entry.path.as_deref() == Some(src.join("lib.rs").as_path()))
        .unwrap();
    tree.set_selected(selected_ix);
    let selected_path = tree.selected().and_then(|entry| entry.path.clone());
    let lib = src.join("lib.rs");
    let new_file = src.join("new.rs");
    assert!(tree
        .entries()
        .iter()
        .any(|entry| { entry.path.as_deref() == Some(lib.as_path()) }));

    std::fs::write(&new_file, "").unwrap();
    tree.populate_from_dir(&root);

    assert_eq!(
        tree.selected().and_then(|entry| entry.path.clone()),
        selected_path
    );
    assert!(tree
        .entries()
        .iter()
        .any(|entry| { entry.path.as_deref() == Some(lib.as_path()) }));
    assert!(tree
        .entries()
        .iter()
        .any(|entry| { entry.path.as_deref() == Some(new_file.as_path()) }));
    let src_entry = tree
        .entries()
        .iter()
        .find(|entry| entry.path.as_deref() == Some(src.as_path()))
        .unwrap();
    assert_eq!(src_entry.kind, NodeKind::Dir { open: true });

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refresh_after_file_edit_preserves_open_dirs_from_state() {
    let root = std::env::temp_dir()
        .join(format!("neoism-tree-refresh-edit-{}", std::process::id()));
    let src = root.join("src");
    let nested = src.join("nested");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&nested).unwrap();
    let edited = nested.join("main.rs");
    std::fs::write(&edited, "one").unwrap();

    let mut tree = FileTree::new();
    tree.populate_from_dir(&root);
    let src_ix = tree
        .entries()
        .iter()
        .position(|entry| entry.path.as_deref() == Some(src.as_path()))
        .unwrap();
    tree.toggle_dir_at(src_ix);
    let nested_ix = tree
        .entries()
        .iter()
        .position(|entry| entry.path.as_deref() == Some(nested.as_path()))
        .unwrap();
    tree.toggle_dir_at(nested_ix);

    std::fs::write(&edited, "two").unwrap();
    tree.refresh();

    assert!(tree.entries().iter().any(|entry| {
        entry.path.as_deref() == Some(src.as_path())
            && entry.kind == (NodeKind::Dir { open: true })
    }));
    assert!(tree.entries().iter().any(|entry| {
        entry.path.as_deref() == Some(nested.as_path())
            && entry.kind == (NodeKind::Dir { open: true })
    }));
    assert!(tree
        .entries()
        .iter()
        .any(|entry| entry.path.as_deref() == Some(edited.as_path())));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn porcelain_status_parses_and_marks_parents() {
    let root = PathBuf::from("/tmp/repo");
    let statuses = parse_git_status(
        &root,
        b" M src/main.rs\0M  src/staged.rs\0MM src/mixed.rs\0?? README.md\0A  src/lib.rs\0",
    );

    assert_eq!(
        statuses.get(&PathBuf::from("/tmp/repo/src/main.rs")),
        Some(&GitStatus::Modified)
    );
    assert_eq!(
        statuses.get(&PathBuf::from("/tmp/repo/src/staged.rs")),
        Some(&GitStatus::StagedModified)
    );
    assert_eq!(
        statuses.get(&PathBuf::from("/tmp/repo/src/mixed.rs")),
        Some(&GitStatus::Mixed)
    );
    assert_eq!(
        statuses.get(&PathBuf::from("/tmp/repo/README.md")),
        Some(&GitStatus::Untracked)
    );
    assert_eq!(
        statuses.get(&PathBuf::from("/tmp/repo/src/lib.rs")),
        Some(&GitStatus::Added)
    );
    assert_eq!(
        statuses.get(&PathBuf::from("/tmp/repo/src")),
        Some(&GitStatus::Added)
    );
}

#[test]
fn deleted_git_paths_stay_visible_as_ghost_entries() {
    let root =
        std::env::temp_dir().join(format!("neoism-tree-deleted-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let statuses = parse_git_status(&root, b" D old.rs\0 D gone/nested.rs\0");

    let entries = scan_dir(&root, 0, &statuses);
    assert!(entries.iter().any(|entry| {
        entry.label == "old.rs"
            && entry.kind == NodeKind::File
            && entry.git_status == GitStatus::Deleted
    }));
    assert!(entries.iter().any(|entry| {
        entry.label == "gone"
            && entry.kind == (NodeKind::Dir { open: false })
            && entry.git_status == GitStatus::Deleted
    }));

    let open_dirs = HashSet::from([normalize_path(&root.join("gone"))]);
    let opened = scan_dir_with_open(&root, 0, &statuses, &open_dirs);
    assert!(opened.iter().any(|entry| {
        entry.label == "gone" && entry.kind == (NodeKind::Dir { open: true })
    }));
    assert!(opened.iter().any(|entry| {
        entry.label == "nested.rs"
            && entry.depth == 1
            && entry.git_status == GitStatus::Deleted
    }));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn git_status_markers_distinguish_staged_and_unstaged() {
    assert_eq!(GitStatus::Modified.marker(), Some("M"));
    assert_eq!(GitStatus::StagedModified.marker(), Some("S"));
    assert_eq!(GitStatus::Mixed.marker(), Some("M*"));
    assert_eq!(GitStatus::Deleted.marker(), Some("D"));
}
