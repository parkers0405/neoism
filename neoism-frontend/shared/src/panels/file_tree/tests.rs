use std::collections::HashSet;
use std::path::{Path, PathBuf};
use web_time::Duration;

use crate::panels::PanelContext;
use crate::services::{
    ClipboardService, ClockService, CommandError, CommandService, DirEntry, FilesService,
    GitService, GitStatus as ServiceGitStatus, IoError, NullNotificationService,
    NullSearchService, Services,
};
use crate::theme::ChromeTheme;

use super::git::parse_git_status;
use super::icons::icon_for;
use super::policy::{
    activation_for_selection, close_policy, directory_link_policy, open_command_policy,
    rename_target_for_input, selected_path_for_entry, target_dir_for_selection,
    toggle_visibility_policy, FileTreeBridgeState, RenameTarget, SelectionActivation,
};
use super::scan::{normalize_path, scan_dir, scan_dir_with_open};
use super::state::FileTree;
use super::types::{GitStatus, NodeKind, TreeEntry, VirtualEntryKind};
use super::{FILE_TREE_WIDTH, FOLDER_ICON_COLOR, FRAME_STROKE, ROW_HEIGHT};

/// Test-only `FilesService` impl backed by `std::fs::read_dir`. Mirrors
/// the native `NativeFiles` shim from the editor module; lives here so
/// the cross-platform tests can exercise the same scan/refresh paths
/// the chrome runs on native.
struct StdFiles;

impl FilesService for StdFiles {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, IoError> {
        let read = std::fs::read_dir(path).map_err(map_io)?;
        let mut out = Vec::new();
        for dent in read.flatten() {
            let name = match dent.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let is_dir = dent.file_type().map(|t| t.is_dir()).unwrap_or(false);
            out.push(DirEntry {
                name,
                is_dir,
                size: None,
            });
        }
        Ok(out)
    }
    fn read_file(&self, path: &Path) -> Result<Vec<u8>, IoError> {
        std::fs::read(path).map_err(map_io)
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> Result<(), IoError> {
        std::fs::write(path, bytes).map_err(map_io)
    }
    fn stat(&self, path: &Path) -> Result<DirEntry, IoError> {
        let meta = std::fs::metadata(path).map_err(map_io)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        Ok(DirEntry {
            name,
            is_dir: meta.is_dir(),
            size: Some(meta.len()),
        })
    }
}

fn map_io(err: std::io::Error) -> IoError {
    match err.kind() {
        std::io::ErrorKind::NotFound => IoError::NotFound(err.to_string()),
        std::io::ErrorKind::PermissionDenied => {
            IoError::PermissionDenied(err.to_string())
        }
        _ => IoError::Other(err.to_string()),
    }
}

/// Test-only `GitService` impl that returns "no repo" for everything.
/// The file_tree tests never assert on git overlays, so this is enough.
struct NullGit;
impl GitService for NullGit {
    fn status(&self, _repo: &Path) -> Result<ServiceGitStatus, IoError> {
        Ok(ServiceGitStatus {
            branch: None,
            dirty: false,
        })
    }
    fn diff(&self, _repo: &Path, _path: Option<&Path>) -> Result<String, IoError> {
        Ok(String::new())
    }
}

struct NullClipboard;
impl ClipboardService for NullClipboard {
    fn read(&self) -> Option<String> {
        None
    }
    fn write(&self, _text: &str) {}
}

struct NullCommands;
impl CommandService for NullCommands {
    fn run(&self, _command: &str) -> Result<(), CommandError> {
        Ok(())
    }
}

struct NullClock;
impl ClockService for NullClock {
    fn now_monotonic(&self) -> Duration {
        Duration::ZERO
    }
}

/// Build a fresh `PanelContext` backed by the stdlib services above.
/// Each call constructs a fresh theme; callers don't keep the context
/// across frames in the tests.
fn with_ctx<R>(f: impl FnOnce(&PanelContext) -> R) -> R {
    let files = StdFiles;
    let clipboard = NullClipboard;
    let commands = NullCommands;
    let git = NullGit;
    let clock = NullClock;
    let search = NullSearchService;
    let notifications = NullNotificationService;
    let theme = ChromeTheme::default();
    let services = Services {
        files: &files,
        clipboard: &clipboard,
        commands: &commands,
        git: &git,
        clock: &clock,
        search: &search,
        notifications: &notifications,
    };
    let ctx = PanelContext {
        services,
        theme: &theme,
        time: Duration::ZERO,
    };
    f(&ctx)
}

/// Mutable variant of `with_ctx` — same services, but the caller gets a
/// `&mut PanelContext`. Used for `toggle_dir_at` / `populate_from_dir`
/// which only need read-only services anyway.
fn with_ctx_mut<R>(f: impl FnOnce(&mut PanelContext) -> R) -> R {
    let files = StdFiles;
    let clipboard = NullClipboard;
    let commands = NullCommands;
    let git = NullGit;
    let clock = NullClock;
    let search = NullSearchService;
    let notifications = NullNotificationService;
    let theme = ChromeTheme::default();
    let services = Services {
        files: &files,
        clipboard: &clipboard,
        commands: &commands,
        git: &git,
        clock: &clock,
        search: &search,
        notifications: &notifications,
    };
    let mut ctx = PanelContext {
        services,
        theme: &theme,
        time: Duration::ZERO,
    };
    f(&mut ctx)
}

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

fn policy_entry(
    label: &str,
    kind: NodeKind,
    path: Option<&str>,
    virtual_kind: Option<VirtualEntryKind>,
) -> TreeEntry {
    TreeEntry {
        label: label.into(),
        depth: 0,
        kind,
        path: path.map(PathBuf::from),
        git_status: GitStatus::None,
        virtual_kind,
    }
}

#[test]
fn visibility_policy_matches_desktop_toggle_cycle() {
    let opening = toggle_visibility_policy(FileTreeBridgeState {
        visible: false,
        focused: false,
    });
    assert!(opening.visible);
    assert!(opening.focused);
    assert!(opening.visibility_changed);
    assert!(opening.refresh_workspace_root);

    let focusing = toggle_visibility_policy(FileTreeBridgeState {
        visible: true,
        focused: false,
    });
    assert!(focusing.visible);
    assert!(focusing.focused);
    assert!(!focusing.visibility_changed);
    assert!(!focusing.refresh_workspace_root);

    let closing = toggle_visibility_policy(FileTreeBridgeState {
        visible: true,
        focused: true,
    });
    assert!(!closing.visible);
    assert!(!closing.focused);
    assert!(closing.visibility_changed);
    assert!(!closing.refresh_workspace_root);
}

#[test]
fn open_and_close_policy_report_layout_changes() {
    let already_open = open_command_policy(FileTreeBridgeState {
        visible: true,
        focused: false,
    });
    assert!(already_open.visible);
    assert!(already_open.focused);
    assert!(!already_open.visibility_changed);
    assert!(!already_open.refresh_workspace_root);

    let opening = open_command_policy(FileTreeBridgeState {
        visible: false,
        focused: false,
    });
    assert!(opening.visibility_changed);
    assert!(opening.refresh_workspace_root);

    assert!(close_policy(FileTreeBridgeState {
        visible: false,
        focused: false,
    })
    .is_none());
    let closing = close_policy(FileTreeBridgeState {
        visible: true,
        focused: true,
    })
    .unwrap();
    assert!(!closing.visible);
    assert!(!closing.focused);
    assert!(closing.visibility_changed);
}

#[test]
fn directory_link_policy_prefers_existing_containing_roots() {
    let decision = directory_link_policy(
        Path::new("/repo/src/bin"),
        Some(Path::new("/repo")),
        Some(Path::new("/other")),
        None,
        false,
    );

    assert_eq!(decision.reveal_root, PathBuf::from("/repo"));
    assert!(decision.visible);
    assert!(decision.focused);
    assert!(decision.visibility_changed);
}

#[test]
fn directory_link_policy_falls_back_to_parent() {
    let decision =
        directory_link_policy(Path::new("/loose/folder"), None, None, None, true);

    assert_eq!(decision.reveal_root, PathBuf::from("/loose"));
    assert!(!decision.visibility_changed);
}

#[test]
fn selection_policy_classifies_open_toggle_and_virtual_actions() {
    let file = policy_entry("main.rs", NodeKind::File, Some("/repo/src/main.rs"), None);
    assert_eq!(
        activation_for_selection(Some(&file), 3),
        SelectionActivation::OpenPath(PathBuf::from("/repo/src/main.rs"))
    );

    let dir = policy_entry(
        "src",
        NodeKind::Dir { open: false },
        Some("/repo/src"),
        None,
    );
    assert_eq!(
        activation_for_selection(Some(&dir), 2),
        SelectionActivation::ToggleDirectory { index: 2 }
    );

    let tags = policy_entry("tags", NodeKind::File, None, Some(VirtualEntryKind::Tags));
    assert_eq!(
        activation_for_selection(Some(&tags), 0),
        SelectionActivation::OpenVirtual(VirtualEntryKind::Tags)
    );
}

#[test]
fn target_dir_policy_handles_virtual_roots_and_files() {
    let file = policy_entry("main.rs", NodeKind::File, Some("/repo/src/main.rs"), None);
    assert_eq!(
        target_dir_for_selection(Some(&file), Some(Path::new("/repo")), None),
        Some(PathBuf::from("/repo/src"))
    );

    let workspace = policy_entry(
        "Neoism",
        NodeKind::Dir { open: true },
        Some("/repo/.neoism"),
        Some(VirtualEntryKind::NeoismWorkspace),
    );
    assert_eq!(
        target_dir_for_selection(
            Some(&workspace),
            Some(Path::new("/repo")),
            Some(Path::new("/repo/notes"))
        ),
        Some(PathBuf::from("/repo/notes"))
    );

    let tasks =
        policy_entry("tasks", NodeKind::File, None, Some(VirtualEntryKind::Tasks));
    assert_eq!(
        target_dir_for_selection(Some(&tasks), Some(Path::new("/repo")), None),
        None
    );
}

#[test]
fn selected_path_policy_ignores_virtual_entries() {
    let file = policy_entry("main.rs", NodeKind::File, Some("/repo/main.rs"), None);
    assert_eq!(
        selected_path_for_entry(Some(&file)),
        Some(PathBuf::from("/repo/main.rs"))
    );

    let virtual_entry = policy_entry(
        "tasks",
        NodeKind::File,
        Some("/repo/tasks.md"),
        Some(VirtualEntryKind::Tasks),
    );
    assert_eq!(selected_path_for_entry(Some(&virtual_entry)), None);
}

#[test]
fn rename_policy_validates_relative_names_without_filesystem_io() {
    let path = Path::new("/repo/src/main.rs");
    assert_eq!(
        rename_target_for_input(path, "lib.rs").unwrap(),
        RenameTarget::Target(PathBuf::from("/repo/src/lib.rs"))
    );
    assert_eq!(
        rename_target_for_input(path, "main.rs").unwrap(),
        RenameTarget::Noop
    );
    assert!(rename_target_for_input(path, "../lib.rs").is_err());
    assert!(rename_target_for_input(path, "/tmp/lib.rs").is_err());
    assert!(rename_target_for_input(path, "   ").is_err());
}

#[test]
fn select_next_clamps_at_end() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(3));
    t.select_next();
    t.select_next();
    t.select_next();
    t.select_next();
    assert_eq!(t.selected_index(), 2);
}

#[test]
fn select_prev_clamps_at_zero() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(3));
    t.select_prev();
    assert_eq!(t.selected_index(), 0);
}

#[test]
fn set_entries_clamps_selection() {
    let mut t = FileTree::empty();
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
fn clamp_scroll_keeps_selection_visible() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(40));
    // Window of 5 rows, jump to row 20.
    for _ in 0..20 {
        t.select_next();
    }
    t.clamp_scroll(5);
    assert!(t.scroll_top <= 20);
    assert!(t.scroll_top + 5 > 20);
    // Move back up: scroll should follow.
    for _ in 0..18 {
        t.select_prev();
    }
    t.clamp_scroll(5);
    assert!(t.selected_index() >= t.scroll_top);
    assert!(t.selected_index() < t.scroll_top + 5);
}

#[test]
fn hit_test_returns_none_when_hidden() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(3));
    assert_eq!(t.hit_test(10.0, 10.0, 0.0, 200.0), None);
}

#[test]
fn hit_test_maps_y_to_row() {
    let mut t = FileTree::empty();
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
    let mut t = FileTree::empty();
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
    let mut t = FileTree::empty();
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
    with_ctx_mut(|ctx| {
        t.toggle_dir_at(0, ctx);
    });
    assert_eq!(t.entries().len(), 2);
    assert_eq!(t.entries()[0].label, "A");
    assert_eq!(t.entries()[1].label, "B");
    assert_eq!(t.entries()[0].kind, NodeKind::Dir { open: false });
}

#[test]
fn toggle_on_file_is_noop() {
    let mut t = FileTree::empty();
    t.set_entries(vec![TreeEntry {
        label: "f.rs".into(),
        depth: 0,
        kind: NodeKind::File,
        path: Some(PathBuf::from("/tmp/f.rs")),
        git_status: GitStatus::None,
        virtual_kind: None,
    }]);
    let result = with_ctx_mut(|ctx| t.toggle_dir_at(0, ctx));
    assert!(result.is_none());
    assert_eq!(t.entries().len(), 1);
}

#[test]
fn expanding_dir_preserves_visible_scroll_position() {
    let root = std::env::temp_dir().join(format!(
        "neoism-ui-tree-expand-scroll-{}",
        std::process::id()
    ));
    let dir = root.join("open_me");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("child_a.rs"), "").unwrap();
    std::fs::write(dir.join("child_b.rs"), "").unwrap();

    let mut entries = sample_entries(30);
    entries[15] = TreeEntry {
        label: "open_me".into(),
        depth: 0,
        kind: NodeKind::Dir { open: false },
        path: Some(dir.clone()),
        git_status: GitStatus::None,
        virtual_kind: None,
    };

    let mut t = FileTree::empty();
    t.set_entries(entries);
    t.last_panel_height_rows = 8;
    t.scroll_top = 11;
    t.selected = 15;

    let old_scroll_top = t.scroll_top;
    with_ctx_mut(|ctx| {
        t.toggle_dir_at(15, ctx);
    });

    assert_eq!(t.scroll_top, old_scroll_top);
    assert_eq!(t.entries()[15].kind, NodeKind::Dir { open: true });
    assert!(t.entries().len() > 30);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reveal_directory_expands_selects_and_flashes_target() {
    let root = std::env::temp_dir()
        .join(format!("neoism-ui-tree-reveal-{}", std::process::id()));
    let target = root.join("src").join("bin");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("main.rs"), "").unwrap();

    let mut t = FileTree::empty();
    let ix = with_ctx_mut(|ctx| {
        t.populate_from_dir(&root, ctx);
        t.reveal_directory(&target, ctx)
    })
    .unwrap();

    assert_eq!(t.selected_index(), ix);
    assert_eq!(t.entries()[ix].path.as_deref(), Some(target.as_path()));
    assert_eq!(t.entries()[ix].kind, NodeKind::Dir { open: true });
    assert!(t
        .reveal_flash
        .as_ref()
        .is_some_and(|flash| flash.index == ix));

    let _ = std::fs::remove_dir_all(root);
}

// `workspace_virtual_root_starts_collapsed_and_refresh_preserves_toggle`
// removed: the virtual "Neoism" workspace root in the file tree was
// deliberately gutted in 60be4fe4 (virtuals.rs is now a stub and the panel no
// longer pulls in `neoism_workspace_index`). Restore alongside the feature if
// it comes back.

#[test]
fn hit_test_returns_none_past_last_row() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(2));
    t.set_visible(true);
    let y_far = ROW_HEIGHT * 5.0;
    assert_eq!(t.hit_test(20.0, y_far, 0.0, 400.0), None);
}

#[test]
fn touchpad_scroll_accumulates_away_from_top_edge() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(20));
    t.set_visible(true);

    for _ in 0..8 {
        t.scroll_pixels(-ROW_HEIGHT / 4.0, 5);
    }

    assert_eq!(t.scroll_top, 2);
}

#[test]
fn touchpad_scroll_accumulates_away_from_bottom_edge() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(20));
    t.set_visible(true);
    t.scroll_by(100, 5);
    assert_eq!(t.scroll_top, t.max_scroll_top(5));

    for _ in 0..8 {
        t.scroll_pixels(ROW_HEIGHT / 4.0, 5);
    }

    assert_eq!(t.scroll_top, t.max_scroll_top(5).saturating_sub(2));
}

#[test]
fn tree_scroll_does_not_reserve_virtual_footer_rows() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(20));

    assert_eq!(t.max_scroll_top(5), 15);
    assert_eq!(
        t.visible_rows_for_panel_height(ROW_HEIGHT * 5.0 + FRAME_STROKE * 2.0),
        5
    );
}

#[test]
fn touchpad_overscroll_is_discarded_at_edges() {
    let mut t = FileTree::empty();
    t.set_entries(sample_entries(20));
    t.set_visible(true);

    t.scroll_pixels(ROW_HEIGHT / 2.0, 5);
    t.scroll_pixels(-ROW_HEIGHT / 2.0, 5);

    assert_eq!(t.scroll_top, 0);
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
    let root = std::env::temp_dir()
        .join(format!("neoism-ui-tree-deleted-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let statuses = parse_git_status(&root, b" D old.rs\0 D gone/nested.rs\0");

    let files = StdFiles;
    let entries = scan_dir(&root, 0, &statuses, &files);
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
    let opened = scan_dir_with_open(&root, 0, &statuses, &open_dirs, &files);
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

// Keep the `with_ctx` helper alive even when its const-time variant
// isn't exercised below — the test fixture is part of the file_tree
// surface area and other tests may grow into it.
#[test]
fn with_ctx_helper_is_callable() {
    with_ctx(|_ctx| {});
}
