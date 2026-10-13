use std::path::Path;
use std::sync::{Arc, Mutex};

use super::types::{GitAction, Rect};
use super::{FileChange, FileStatus, GitDiffIo, GitDiffPanel, PanelHit};

#[derive(Default)]
struct HostIo(Mutex<Vec<String>>);

impl HostIo {
    fn record(&self, root: &Path, action: &str) -> Result<(), String> {
        assert_eq!(root, Path::new("/remote/repo"));
        self.0.lock().unwrap().push(action.into());
        Ok(())
    }
}

impl GitDiffIo for HostIo {
    fn is_host_driven(&self) -> bool {
        true
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
    fn commit(&self, root: &Path, _: &str) -> Result<(), String> {
        self.record(root, "commit")
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
        Some(&GitAction::Refresh)
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
