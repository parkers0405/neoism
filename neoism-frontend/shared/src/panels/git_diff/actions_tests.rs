use std::path::Path;
use std::sync::{Arc, Mutex};

use super::types::{GitAction, Rect};
use super::{FileChange, FileStatus, GitDiffIo, GitDiffPanel, PanelHit};

#[derive(Default)]
struct HostIo(Mutex<Vec<String>>, bool, bool);

impl HostIo {
    fn record(&self, root: &Path, action: &str) -> Result<(), String> {
        assert_eq!(root, Path::new("/remote/repo"));
        self.0.lock().unwrap().push(action.into());
        Ok(())
    }
}

impl GitDiffIo for HostIo {
    fn request_base(&self, path: &str) {
        self.0.lock().unwrap().push(format!("base:{path}"));
    }

    fn request_diffs(&self, paths: &[String]) {
        self.0
            .lock()
            .unwrap()
            .push(format!("diffs:{}", paths.join(",")));
    }

    fn is_host_driven(&self) -> bool {
        !self.1
    }
    fn upstream(&self, _: &Path) -> Option<String> {
        Some("origin/main".into())
    }
    fn collect_files(&self, root: &Path) -> Vec<FileChange> {
        self.record(root, "refresh").unwrap();
        vec![]
    }
    fn stage(&self, root: &Path, _: &str) -> Result<(), String> {
        self.record(root, "stage")
    }
    fn unstage(&self, root: &Path, _: &str) -> Result<(), String> {
        self.record(root, "unstage")
    }
    fn restore(&self, root: &Path, _: &str) -> Result<(), String> {
        self.record(root, "restore")
    }
    fn apply_block(
        &self,
        root: &Path,
        path: &str,
        block: &neoism_protocol::git::GitChangeBlock,
        restore: bool,
    ) -> Result<(), String> {
        self.record(
            root,
            &format!(
                "block:{path}:{restore}:{}",
                serde_json::to_string(block).unwrap()
            ),
        )
    }
    fn commit(&self, root: &Path, _: &str) -> Result<(), String> {
        self.record(root, "commit")?;
        if self.2 {
            Err("Commit failed".into())
        } else {
            Ok(())
        }
    }
    fn list_branches(&self, root: &Path) -> Vec<String> {
        self.record(root, "branches").unwrap();
        vec![]
    }
    fn checkout(&self, root: &Path, _: &str) -> Result<(), String> {
        self.record(root, "checkout")
    }
    fn fetch(&self, root: &Path) -> Result<(), String> {
        self.record(root, "fetch")
    }
    fn pull(&self, root: &Path) -> Result<(), String> {
        self.record(root, "pull")
    }
    fn push(&self, root: &Path) -> Result<(), String> {
        self.record(root, "push")
    }
    fn publish(&self, root: &Path) -> Result<(), String> {
        self.record(root, "publish")
    }
}

#[test]
fn block_mutations_preserve_exact_targets_and_never_use_whole_file_verbs() {
    let (mut panel, io) = panel();
    let files = vec![FileChange {
        path: "mixed.rs".into(),
        status: FileStatus::Mixed,
        additions: 2,
        deletions: 2,
        staged: true,
    }];
    panel.host_set_files(files.clone());
    let block = neoism_protocol::git::GitChangeBlock {
        old_start: 10,
        new_start: 12,
        old_lines: vec!["old".into()],
        new_lines: vec!["new".into()],
    };
    assert!(!panel.apply_block(Path::new("/other"), "mixed.rs", block.clone(), false));
    assert!(!panel.apply_block(Path::new("/remote/repo"), "", block.clone(), false));
    assert!(io.0.lock().unwrap().is_empty());
    for restore in [false, true] {
        assert!(panel.apply_block(
            Path::new("/remote/repo"),
            "mixed.rs",
            block.clone(),
            restore
        ));
        assert!(!panel.apply_block(
            Path::new("/remote/repo"),
            "mixed.rs",
            block.clone(),
            restore
        ));
        assert_eq!(
            io.0.lock().unwrap().last().unwrap(),
            &format!(
                "block:mixed.rs:{restore}:{}",
                serde_json::to_string(&block).unwrap(),
            )
        );
        panel.host_set_files(files.clone());
    }
    assert_eq!(io.0.lock().unwrap().len(), 2);
    // Status may be clean after Restore while the retained source has new edits.
    panel.host_set_files(Vec::new());
    assert!(panel.apply_block(
        Path::new("/remote/repo"),
        "mixed.rs",
        block.clone(),
        false
    ));
    assert_eq!(io.0.lock().unwrap().len(), 3);
    panel.host_set_files(Vec::new());
    let empty = neoism_protocol::git::GitChangeBlock {
        old_start: 0,
        new_start: 0,
        old_lines: vec![],
        new_lines: vec![],
    };
    assert!(!panel.apply_block(Path::new("/remote/repo"), "mixed.rs", empty, false));
}

#[test]
fn review_actions_revalidate_root_status_and_loading() {
    let (mut panel, io) = panel();
    let file = FileChange {
        path: "mixed.rs".into(),
        status: FileStatus::Mixed,
        additions: 1,
        deletions: 1,
        staged: true,
    };
    panel.host_set_files(vec![file.clone()]);
    panel.stage_path(Path::new("/other/repo"), "mixed.rs", false);
    panel.restore_file(Path::new("/other/repo"), "mixed.rs");
    panel.stage_path(Path::new("/remote/repo"), "mixed.rs", true);
    assert!(io.0.lock().unwrap().is_empty());
    panel.stage_path(Path::new("/remote/repo"), "mixed.rs", false);
    assert_eq!(*io.0.lock().unwrap(), vec!["stage"]);
    panel.restore_file(Path::new("/remote/repo"), "mixed.rs");
    assert_eq!(io.0.lock().unwrap().len(), 1); // mutation still loading
    panel.host_set_files(vec![FileChange {
        status: FileStatus::Untracked,
        staged: false,
        ..file.clone()
    }]);
    panel.restore_file(Path::new("/remote/repo"), "mixed.rs");
    assert_eq!(io.0.lock().unwrap().len(), 1);
    panel.host_set_files(vec![file]);
    panel.restore_file(Path::new("/remote/repo"), "mixed.rs");
    assert_eq!(*io.0.lock().unwrap(), vec!["stage", "restore"]);
}

#[test]
fn diff_tab_snapshots_resolve_foreign_paths_and_clear_after_commit() {
    let (mut panel, _) = panel();
    panel.host_set_repo(Some(r"C:\host\repo".into()), Some("main".into()));
    panel.host_set_diff_text("a.rs", "@@ -1 +1 @@\n-old\n+new\n");
    let lines = panel
        .diff_lines_for_path(Path::new(r"C:\host\repo\a.rs"))
        .unwrap();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].kind, crate::widgets::diff_card::DiffLineKind::Hunk);
    panel.host_set_files(vec![]);
    assert!(panel
        .diff_lines_for_path(Path::new("a.rs"))
        .unwrap()
        .is_empty());
    panel.host_set_error("Disconnected".into());
    assert!(panel.diff_lines_for_path(Path::new("a.rs")).is_none());
}

#[test]
fn repository_review_loads_missing_patches_in_one_single_flight_batch() {
    let (mut panel, io) = panel();
    let expected = panel.files().len();
    panel.request_review_diffs();
    panel.request_review_diffs();
    let calls = io.0.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].strip_prefix("diffs:").unwrap().split(',').count(),
        expected
    );
    for file in panel.files() {
        panel.host_set_diff_text(&file.path, "");
    }
    panel.request_review_diffs();
    assert_eq!(io.0.lock().unwrap().len(), 1);
    panel.host_set_files(panel.files());
    panel.request_review_diffs();
    assert_eq!(io.0.lock().unwrap().len(), 2);
}

#[test]
fn editable_review_baselines_are_lazy_scoped_and_invalidated_with_status() {
    let (mut panel, io) = panel();
    panel.host_set_repo(Some(r"C:\host\repo".into()), Some("main".into()));
    panel.request_diff_base(Path::new(r"C:\host\repo\a.rs"));
    panel.request_diff_base(Path::new("a.rs"));
    assert_eq!(io.0.lock().unwrap().as_slice(), &["base:a.rs"]);
    assert!(panel.diff_base_pending(Path::new("a.rs")));
    panel.host_set_base_text("a.rs", "exact HEAD\r\n".into());
    assert!(!panel.diff_base_pending(Path::new("a.rs")));
    assert_eq!(
        panel
            .diff_base_for_path(Path::new(r"C:\host\repo\a.rs"))
            .as_deref(),
        Some("exact HEAD\r\n")
    );
    panel.host_set_files(panel.files());
    assert!(panel.diff_base_for_path(Path::new("a.rs")).is_none());
    panel.request_diff_base(Path::new("a.rs"));
    assert_eq!(io.0.lock().unwrap().len(), 2);
    panel.host_set_base_text("a.rs", String::new());
    assert_eq!(
        panel.diff_base_for_path(Path::new("a.rs")).as_deref(),
        Some("")
    );
    panel.host_set_files(panel.files());
    panel.request_diff_base(Path::new("a.rs"));
    panel.host_set_error("Cannot edit binary source".into());
    assert_eq!(
        panel.diff_base_error(Path::new("a.rs")).as_deref(),
        Some("Cannot edit binary source")
    );
    assert!(!panel.diff_base_pending(Path::new("a.rs")));
}

fn panel() -> (GitDiffPanel, Arc<HostIo>) {
    let io = Arc::new(HostIo::default());
    let mut panel = GitDiffPanel::new();
    panel.set_io_provider(io.clone());
    panel.open(Some("/remote/repo".into()), Some("main".into()));
    panel.host_set_files(vec![
        FileChange {
            path: "a.rs".into(),
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            staged: false,
        },
        FileChange {
            path: "b.rs".into(),
            status: FileStatus::Modified,
            additions: 1,
            deletions: 0,
            staged: true,
        },
    ]);
    panel.focus_commit_box(true);
    panel.commit_input_insert("Keep this message");
    io.0.lock().unwrap().clear();
    (panel, io)
}

#[test]
fn menu_dispatches_every_supported_action_through_host_provider() {
    for (slot, expected) in [
        "commit", "stage", "unstage", "stage", "fetch", "pull", "push", "branches",
        "refresh",
    ]
    .into_iter()
    .enumerate()
    {
        let (mut panel, io) = panel();
        panel.open_actions_menu();
        panel.activate_action_row(slot);
        assert_eq!(*io.0.lock().unwrap(), [expected], "menu slot {slot}");
        assert!(!panel.actions_menu_is_open());
        assert_eq!(panel.branch_menu_is_open(), expected == "branches");
    }
}

#[test]
fn unavailable_and_stale_actions_cannot_mutate_or_clear_commit_draft() {
    let (mut panel, io) = panel();
    panel.open_actions_menu();
    panel.host_set_files(vec![]);
    panel.activate_action_row(0);
    assert!(io.0.lock().unwrap().is_empty());
    assert_eq!(panel.commit_input_text(), "Keep this message");
    panel.refresh_actions_menu();
    assert!(!panel.git_action_enabled(GitAction::StageAll));
    assert!(!panel.git_action_enabled(GitAction::UnstageAll));
    assert!(panel.git_action_enabled(GitAction::Push));
    panel.data.lock().unwrap().loading = true;
    panel.activate_action_row(6);
    assert!(io.0.lock().unwrap().is_empty());
    panel.commit();
    assert_eq!(panel.commit_input_text(), "Keep this message");
    panel.data.lock().unwrap().loading = false;
    panel.host_set_repo(None, None);
    assert!(GitAction::ALL
        .into_iter()
        .all(|action| !panel.git_action_enabled(action)));
    panel.actions_menu_activate();
    assert!(io.0.lock().unwrap().is_empty());
}

#[test]
fn git_panel_default_width_matches_the_unified_left_sidebar() {
    let panel = GitDiffPanel::new();
    let left = crate::panels::left_sidebar_host::LeftSidebarHost::default();
    assert_eq!(panel.width(), left.unified_width());
}

#[test]
fn menu_keyboard_skips_disabled_rows_and_follows_selected_file_state() {
    let (mut panel, _) = panel();
    panel.host_set_files(vec![]);
    panel.open_actions_menu();
    assert_eq!(
        panel.actions_menu.content().selected_action(),
        Some(&GitAction::Fetch)
    );
    panel.actions_menu_move(1);
    assert_eq!(
        panel.actions_menu.content().selected_action(),
        Some(&GitAction::Pull)
    );
    panel.actions_menu_move(-1);
    assert_eq!(
        panel.actions_menu.content().selected_action(),
        Some(&GitAction::Fetch)
    );
    panel.actions_menu_move(-1);
    assert_eq!(
        panel.actions_menu.content().selected_action(),
        Some(&GitAction::BrowsePullRequests)
    );
    panel.host_set_files(vec![FileChange {
        path: "a.rs".into(),
        status: FileStatus::Modified,
        additions: 0,
        deletions: 0,
        staged: true,
    }]);
    panel.refresh_actions_menu();
    assert_eq!(
        panel.actions_menu.content().items()[3].label,
        "Unstage selected file"
    );
    assert!(!panel.actions_menu.content().items()[1].enabled);
}

#[test]
fn popup_owns_covered_rows_and_scroll_and_closes_with_panel_focus() {
    let (mut panel, _) = panel();
    panel.panel_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 300.0,
        h: 500.0,
    };
    panel.open_actions_menu();
    panel.actions_menu_rect = Rect {
        x: 10.0,
        y: 50.0,
        w: 250.0,
        h: 280.0,
    };
    let row = Rect {
        x: 16.0,
        y: 56.0,
        w: 238.0,
        h: 28.0,
    };
    panel.actions_menu_row_rects.push((0, row));
    panel.file_row_rects.push((0, row));
    panel.divider_rect = row;
    panel.files_scrollbar_thumb_rect = row;
    assert!(!panel.is_hovering_divider(20.0, 60.0));
    assert!(!panel.begin_divider_drag(20.0, 60.0));
    assert!(panel.scrollbar_hit(20.0, 60.0).is_none());
    assert!(matches!(
        panel.hit_test(20.0, 60.0),
        PanelHit::ActionMenuRow(0)
    ));
    assert!(matches!(panel.hit_test(12.0, 60.0), PanelHit::Inside));
    panel.actions_menu.content_mut().set_max_visible(3);
    assert!(panel.scroll_at(20.0, 60.0, -28.0));
    assert_eq!(panel.actions_menu.content().scroll_offset(), 1);
    assert!(panel.scroll_touch_at(20.0, 60.0, -28.0));
    assert_eq!(panel.actions_menu.content().scroll_offset(), 2);
    assert_eq!(panel.file_scroll, 0.0);
    panel.open_branch_menu();
    assert!(!panel.actions_menu_is_open());
    panel.open_actions_menu();
    assert!(!panel.branch_menu_is_open());
    panel.focus_commit_box(true);
    assert!(!panel.actions_menu_is_open());
    panel.open_actions_menu();
    panel.set_focused(false);
    assert!(!panel.actions_menu_is_open());
    panel.open_actions_menu();
    panel.close();
    assert!(!panel.actions_menu.is_interactive());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_commit_confirmation_requires_success_and_preserves_newer_draft() {
    for (fail, newer_draft) in [(false, false), (false, true), (true, false)] {
        let (mut panel, _) = panel();
        let io = Arc::new(HostIo(Mutex::new(vec![]), true, fail));
        panel.set_io_provider(io);
        panel.commit();
        if newer_draft {
            panel.commit_input_insert(" updated");
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while panel.loading() {
            assert!(
                std::time::Instant::now() < deadline,
                "native worker timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(panel.upstream().as_deref(), Some("origin/main"));
        assert_eq!(
            panel
                .data
                .lock()
                .unwrap()
                .completed_commit_message
                .is_some(),
            !fail
        );
        panel.apply_commit_confirmation();
        assert_eq!(
            panel.commit_input_text(),
            if newer_draft {
                "Keep this message updated"
            } else if fail {
                "Keep this message"
            } else {
                ""
            }
        );
        assert!(panel
            .data
            .lock()
            .unwrap()
            .completed_commit_message
            .is_none());
    }
}

#[test]
fn mixed_staging_keeps_stage_all_available_for_worktree_remainder() {
    let (mut panel, io) = panel();
    let mut mixed = panel.files().remove(1);
    mixed.status = FileStatus::Mixed;
    assert!(mixed.staged);
    panel.host_set_files(vec![mixed.clone()]);
    assert!(!panel.all_files_staged());
    assert!(panel.git_action_enabled(GitAction::StageAll));
    assert!(panel.git_action_enabled(GitAction::UnstageAll));
    panel.stage_all_toggle();
    assert_eq!(*io.0.lock().unwrap(), ["stage"]);
    panel.host_set_files(vec![mixed]);
    panel.rebuild_visual_rows();
    panel.toggle_folder_stage(0);
    assert_eq!(*io.0.lock().unwrap(), ["stage", "stage"]);
}

#[test]
fn primary_footer_dispatches_publish_or_push_and_respects_loading() {
    let (mut panel, io) = panel();
    assert_eq!(panel.push_button_label(), "Publish");
    panel.activate_push_button();
    assert_eq!(*io.0.lock().unwrap(), ["publish"]);
    panel.activate_push_button();
    assert_eq!(io.0.lock().unwrap().len(), 1);
    panel.host_set_files(vec![]);
    panel.set_upstream(Some("origin/main".into()));
    assert_eq!(panel.push_button_label(), "Push");
    panel.activate_push_button();
    assert_eq!(*io.0.lock().unwrap(), ["publish", "push"]);
    panel.host_set_repo(Some("/remote/repo".into()), Some("new".into()));
    assert_eq!(panel.push_button_label(), "Publish");
}

#[test]
fn footer_regions_are_distinct_clamped_and_hit_testable() {
    for scale in [0.5, 1.0, 2.0, 3.0] {
        for width in [0.0, 40.0, 200.0, 500.0] {
            let [branch, push, actions] =
                super::render::footer_control_rects(10.0, 300.0, width, scale);
            assert!(branch.w >= 0.0 && push.w >= 0.0 && actions.w >= 0.0);
            assert!(branch.x + branch.w <= push.x);
            assert!((push.x + push.w - actions.x).abs() < 0.001);
            assert!((actions.x + actions.w - 10.0 - width).abs() < 0.001);
        }
    }
    let (mut panel, _) = panel();
    panel.panel_rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 300.0,
        h: 500.0,
    };
    let [branch, push, actions] =
        super::render::footer_control_rects(10.0, 300.0, 280.0, 1.0);
    panel.branch_button_rect = branch;
    panel.push_button_rect = push;
    panel.actions_button_rect = actions;
    assert!(matches!(
        panel.hit_test(push.x + 1.0, push.y + 1.0),
        PanelHit::PushButton
    ));
    assert!(matches!(
        panel.hit_test(actions.x + 1.0, actions.y + 1.0),
        PanelHit::ActionsButton
    ));
    assert!(!panel.begin_divider_drag(20.0, 200.0));
}

#[test]
fn focus_order_matches_visible_files_footer_commit_without_diff_stop() {
    use super::types::FocusSection;
    let (mut panel, _) = panel();
    panel.focus_files_section();
    panel.section_focus_next();
    assert_eq!(panel.section, FocusSection::Branch);
    panel.section_focus_next();
    assert_eq!(panel.section, FocusSection::Commit);
    panel.section_focus_prev();
    assert_eq!(panel.section, FocusSection::Branch);
    panel.section_focus_prev();
    assert_eq!(panel.section, FocusSection::Files);
    assert!(!panel.diff_section_focused());
}

#[test]
fn groups_are_tracked_then_untracked_and_collapse_skips_navigation() {
    use super::types::VisualRowKind;
    let (mut panel, _) = panel();
    let mut files = panel.files();
    files[0].status = FileStatus::Untracked;
    files[0].path = "nested/new.rs".into();
    panel.host_set_files(files);
    panel.rebuild_visual_rows();
    assert!(
        matches!(&panel.visual_rows[0].kind, VisualRowKind::Group { label, .. } if label == "Tracked")
    );
    assert_eq!(panel.visible_leaf_indices(), [1, 0]);
    panel.toggle_folder(0);
    assert_eq!(panel.visible_leaf_indices(), [0]);
    panel.select_first_file();
    assert_eq!(panel.selected_file_index(), Some(0));
}

#[test]
fn nested_tree_counts_collapse_and_keyboard_follow_visible_leaves() {
    use super::types::VisualRowKind;
    let (mut panel, _) = panel();
    let files = [
        ("src/z.rs", FileStatus::Modified),
        ("root.toml", FileStatus::Modified),
        ("src/deep/a.rs", FileStatus::Modified),
        ("src/new.rs", FileStatus::Untracked),
        ("src-other/b.rs", FileStatus::Modified),
    ]
    .into_iter()
    .map(|(path, status)| FileChange {
        path: path.into(),
        status,
        additions: 0,
        deletions: 0,
        staged: false,
    })
    .collect();
    panel.host_set_files(files);
    panel.rebuild_visual_rows();
    assert_eq!(panel.visible_leaf_indices(), [2, 0, 4, 1, 3]);
    assert!(matches!(&panel.visual_rows[1].kind,
        VisualRowKind::Group { path: Some(path), file_indices, .. }
        if path == "src" && file_indices == &[0, 2]));
    assert_eq!(panel.visual_rows[1].depth, 1);
    assert_eq!(panel.visual_rows[3].depth, 3);
    panel.toggle_folder(1);
    assert_eq!(panel.visible_leaf_indices(), [4, 1, 3]);
    panel.select_first_file();
    assert_eq!(panel.selected_file_index(), Some(4));
    panel.select_next();
    assert_eq!(panel.selected_file_index(), Some(1));
    panel.select_last_file();
    assert_eq!(panel.selected_file_index(), Some(3));
    panel.select_prev();
    assert_eq!(panel.selected_file_index(), Some(1));
    panel.toggle_folder(1);
    panel.select_first_file();
    assert_eq!(panel.selected_file_index(), Some(2));
}

#[test]
fn diff_snapshot_generation_and_confirmed_commit_preserve_new_drafts() {
    let (mut panel, _) = panel();
    let generation = panel.diff_cache_generation();
    panel.host_set_diff_text("a.rs", "@@ -1 +1 @@\n-old\n+new\n");
    assert_ne!(panel.diff_cache_generation(), generation);
    assert!(panel
        .diff_lines_for_path(Path::new("/remote/repo/a.rs"))
        .is_some());
    assert!(panel.diff_lines_for_path(Path::new("a.rs")).is_some());
    panel.commit();
    assert_eq!(panel.commit_input_text(), "Keep this message");
    panel.commit_input_insert(" updated");
    panel.host_commit_succeeded("Keep this message");
    assert_eq!(panel.commit_input_text(), "Keep this message updated");
    panel.host_commit_succeeded("Keep this message updated");
    assert_eq!(panel.commit_input_text(), "");
}
