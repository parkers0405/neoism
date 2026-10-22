use super::*;
use crate::panels::git_diff::{FileChange, GitDiffIo};
use std::path::Path;
use std::sync::Arc;

struct ReadOnlyIo;
impl GitDiffIo for ReadOnlyIo {
    fn collect_files(&self, _: &Path) -> Vec<FileChange> {
        panic!("planning must not do IO")
    }
    fn stage(&self, _: &Path, _: &str) -> Result<(), String> {
        panic!("planning must not stage")
    }
    fn unstage(&self, _: &Path, _: &str) -> Result<(), String> {
        panic!("planning must not unstage")
    }
    fn commit(&self, _: &Path, _: &str) -> Result<(), String> {
        panic!("planning must not commit")
    }
    fn list_branches(&self, _: &Path) -> Vec<String> {
        panic!("planning must not do IO")
    }
    fn checkout(&self, _: &Path, _: &str) -> Result<(), String> {
        panic!("planning must not do IO")
    }
    fn fetch(&self, _: &Path) -> Result<(), String> {
        panic!("planning must not do IO")
    }
    fn pull(&self, _: &Path) -> Result<(), String> {
        panic!("planning must not do IO")
    }
    fn push(&self, _: &Path) -> Result<(), String> {
        panic!("planning must not do IO")
    }
}

fn panel() -> GitDiffPanel {
    let mut panel = GitDiffPanel::new();
    panel.set_io_provider(Arc::new(ReadOnlyIo));
    {
        let mut data = panel.data.lock().unwrap();
        data.repo_root = Some("/repo".into());
        data.files = [
            ("src/staged.rs", FileStatus::Staged, true),
            ("src/nested/mixed.rs", FileStatus::Mixed, true),
            ("src/edit.rs", FileStatus::Modified, false),
            ("src/new.rs", FileStatus::Untracked, false),
            ("src-other/outside.rs", FileStatus::Modified, false),
        ]
        .into_iter()
        .map(|(path, status, staged)| FileChange {
            path: path.into(),
            status,
            staged,
            additions: 1,
            deletions: 0,
        })
        .collect();
    }
    panel.rebuild_visual_rows();
    panel
}

fn group(panel: &GitDiffPanel, label: &str) -> usize {
    panel.visual_rows.iter().position(|row| matches!(&row.kind, VisualRowKind::Group { label: name, .. } if name == label)).unwrap()
}

#[test]
fn folder_plan_preserves_tracked_scope_mixed_remainder_and_collapsed_members() {
    let mut panel = panel();
    let src = group(&panel, "Tracked/src");
    let plan = panel.folder_stage_plan(src).unwrap();
    assert_eq!(plan.root, PathBuf::from("/repo"));
    assert!(!plan.unstage);
    assert_eq!(plan.paths, ["src/nested/mixed.rs", "src/edit.rs"]);
    // A collapsed group still addresses precisely its members, not siblings
    // with a common string prefix or files from the Untracked group.
    panel.toggle_folder(src);
    assert_eq!(
        panel
            .folder_stage_plan(group(&panel, "Tracked/src"))
            .unwrap(),
        plan
    );
    let new_files = panel
        .folder_stage_plan(group(&panel, "Untracked/src"))
        .unwrap();
    assert_eq!(new_files.paths, ["src/new.rs"]);
}

#[test]
fn fully_staged_folder_plans_one_exact_unstage_batch() {
    let panel = panel();
    {
        let mut data = panel.data.lock().unwrap();
        data.files[1].status = FileStatus::Staged;
        data.files[2].staged = true;
    }
    let plan = panel
        .folder_stage_plan(group(&panel, "Tracked/src"))
        .unwrap();
    assert!(plan.unstage);
    assert_eq!(
        plan.paths,
        ["src/staged.rs", "src/nested/mixed.rs", "src/edit.rs"]
    );
}

#[test]
fn stage_all_plan_is_a_read_only_exact_snapshot_and_file_toggle_keeps_mixed_semantics() {
    let panel = panel();
    let before_selected = panel.selected;
    let before_loading = panel.loading();
    let plan = panel.stage_all_plan().unwrap();
    assert_eq!(
        plan.paths,
        [
            "src/nested/mixed.rs",
            "src/edit.rs",
            "src/new.rs",
            "src-other/outside.rs"
        ]
    );
    assert!(!plan.unstage);
    // Individual checkbox toggles the checked index side off, even for Mixed;
    // folder/Stage All instead stage the worktree remainder, as before.
    assert!(panel.file_stage_plan(1).unwrap().unstage);
    assert!(!panel.file_stage_plan(2).unwrap().unstage);
    assert_eq!(panel.selected, before_selected);
    assert_eq!(panel.loading(), before_loading);
    assert!(panel.data.lock().unwrap().files[1].staged);
    panel.data.lock().unwrap().files.push(FileChange {
        path: "later.rs".into(),
        status: FileStatus::Modified,
        additions: 1,
        deletions: 0,
        staged: false,
    });
    assert!(!plan.paths.iter().any(|path| path == "later.rs"));
    assert!(panel.file_stage_plan(100).is_none());
}

#[test]
fn menu_peek_uses_real_actions_without_executing_or_reselecting_them() {
    let mut panel = panel();
    panel.selected = 1;
    panel.refresh_actions_menu();
    let slot = |panel: &GitDiffPanel, action| {
        panel
            .actions_menu
            .content()
            .items()
            .iter()
            .position(|item| item.action == action)
            .unwrap()
    };
    let stage_all = slot(&panel, GitAction::StageAll);
    let toggle = slot(&panel, GitAction::ToggleStageSelected);
    let refresh = slot(&panel, GitAction::Refresh);
    let before = panel.actions_menu.content().selected_index();
    assert_eq!(
        panel.action_stage_plan(Some(stage_all)),
        panel.stage_all_plan()
    );
    assert_eq!(
        panel.action_stage_plan(Some(toggle)),
        panel.selected_stage_plan()
    );
    assert!(panel.action_stage_plan(Some(refresh)).is_none());
    assert_eq!(panel.actions_menu.content().selected_index(), before);
    panel.actions_menu.content_mut().set_selected_index(toggle);
    assert_eq!(panel.action_stage_plan(None), panel.selected_stage_plan());
    panel.data.lock().unwrap().loading = true;
    assert!(panel.action_stage_plan(Some(stage_all)).is_none());
}
