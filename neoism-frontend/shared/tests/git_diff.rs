//! Behavior tests for the slim git-diff panel.
//!
//! Covers:
//! - Hidden panel ignores events.
//! - `set_files` populates state and clamps selection.
//! - ArrowDown / ArrowUp move selection within bounds.
//! - Escape hides.
//! - `UiEvent::ServiceReply` decodes a JSON payload back into files.

use std::path::Path;
use std::time::Duration;

use neoism_ui::event::{
    KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PhysicalKey, UiEvent,
};
use neoism_ui::panels::{DiffFile, DiffHunk, DiffLine, GitDiff, Panel, PanelContext};
use neoism_ui::services::{
    ClipboardService, ClockService, CommandError, CommandService, DirEntry, FilesService,
    GitService, GitStatus, IoError, Services,
};
use neoism_ui::theme::ChromeTheme;

struct NullFiles;
impl FilesService for NullFiles {
    fn list_dir(&self, _path: &Path) -> Result<Vec<DirEntry>, IoError> {
        Ok(Vec::new())
    }
    fn read_file(&self, _path: &Path) -> Result<Vec<u8>, IoError> {
        Err(IoError::NotFound("test".into()))
    }
    fn write_file(&self, _path: &Path, _bytes: &[u8]) -> Result<(), IoError> {
        Ok(())
    }
    fn stat(&self, _path: &Path) -> Result<DirEntry, IoError> {
        Err(IoError::NotFound("test".into()))
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

struct NullGit;
impl GitService for NullGit {
    fn status(&self, _repo: &Path) -> Result<GitStatus, IoError> {
        Ok(GitStatus {
            branch: None,
            dirty: false,
        })
    }
    fn diff(&self, _repo: &Path, _path: Option<&Path>) -> Result<String, IoError> {
        Ok(String::new())
    }
}

/// Returns `IoError::Pending(42)` on every `diff` call so the panel
/// records a request id we can match against in the ServiceReply
/// flow.
struct PendingGit {
    req_id: u64,
}
impl GitService for PendingGit {
    fn status(&self, _repo: &Path) -> Result<GitStatus, IoError> {
        Ok(GitStatus {
            branch: None,
            dirty: false,
        })
    }
    fn diff(&self, _repo: &Path, _path: Option<&Path>) -> Result<String, IoError> {
        Err(IoError::Pending(self.req_id))
    }
}

struct FixedClock;
impl ClockService for FixedClock {
    fn now_monotonic(&self) -> Duration {
        Duration::from_millis(0)
    }
}

struct Harness<G: GitService> {
    files: NullFiles,
    clipboard: NullClipboard,
    commands: NullCommands,
    git: G,
    clock: FixedClock,
    theme: ChromeTheme,
}

impl Harness<NullGit> {
    fn new() -> Self {
        Self {
            files: NullFiles,
            clipboard: NullClipboard,
            commands: NullCommands,
            git: NullGit,
            clock: FixedClock,
            theme: ChromeTheme::default(),
        }
    }
}

impl Harness<PendingGit> {
    fn with_pending(req_id: u64) -> Self {
        Self {
            files: NullFiles,
            clipboard: NullClipboard,
            commands: NullCommands,
            git: PendingGit { req_id },
            clock: FixedClock,
            theme: ChromeTheme::default(),
        }
    }
}

impl<G: GitService> Harness<G> {
    fn run<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut PanelContext) -> R,
    {
        let services = Services {
            files: &self.files,
            clipboard: &self.clipboard,
            commands: &self.commands,
            git: &self.git,
            clock: &self.clock,
            search: &neoism_ui::services::NullSearchService,
            notifications: &neoism_ui::services::NullNotificationService,
        };
        let mut ctx = PanelContext {
            services,
            theme: &self.theme,
            time: Duration::from_millis(0),
            plugins: None,
        };
        f(&mut ctx)
    }
}

fn press(named: NamedKey) -> UiEvent {
    UiEvent::Key(KeyDescriptor {
        physical: PhysicalKey(0),
        logical: LogicalKey::Named(named),
        state: KeyState::Pressed,
        modifiers: Modifiers::empty(),
        repeat: false,
    })
}

fn sample_files() -> Vec<DiffFile> {
    vec![
        DiffFile {
            path: "src/lib.rs".to_string(),
            hunks: vec![DiffHunk {
                old_start: 10,
                new_start: 10,
                lines: vec![
                    DiffLine::Context("// header".to_string()),
                    DiffLine::Removed("let old = 1;".to_string()),
                    DiffLine::Added("let new_value = 2;".to_string()),
                ],
            }],
            added: 1,
            removed: 1,
        },
        DiffFile {
            path: "src/main.rs".to_string(),
            hunks: vec![DiffHunk {
                old_start: 1,
                new_start: 1,
                lines: vec![DiffLine::Added("fn main() {}".to_string())],
            }],
            added: 1,
            removed: 0,
        },
        DiffFile {
            path: "README.md".to_string(),
            hunks: Vec::new(),
            added: 0,
            removed: 5,
        },
    ]
}

#[test]
fn starts_hidden_with_no_files() {
    let panel = GitDiff::new();
    assert!(!panel.is_visible());
    assert_eq!(panel.files().len(), 0);
    assert!(panel.selected_index().is_none());
    assert!(panel.selected_path().is_none());
    assert!(!panel.is_pending());
}

#[test]
fn hidden_panel_ignores_events() {
    let harness = Harness::new();
    let mut panel = GitDiff::new();
    panel.set_files(sample_files());

    harness.run(|ctx| {
        // Events delivered while hidden must not move selection.
        panel.handle_event(&press(NamedKey::ArrowDown), ctx);
        panel.handle_event(&press(NamedKey::ArrowDown), ctx);
    });
    assert!(!panel.is_visible());
    // Selection started at 0 and should still be 0.
    assert_eq!(panel.selected_index(), Some(0));
}

#[test]
fn set_files_then_navigate() {
    let harness = Harness::new();
    let mut panel = GitDiff::new();
    panel.set_files(sample_files());
    panel.show();

    assert_eq!(panel.selected_path(), Some("src/lib.rs"));

    harness.run(|ctx| {
        panel.handle_event(&press(NamedKey::ArrowDown), ctx);
    });
    assert_eq!(panel.selected_path(), Some("src/main.rs"));

    harness.run(|ctx| {
        panel.handle_event(&press(NamedKey::ArrowDown), ctx);
    });
    assert_eq!(panel.selected_path(), Some("README.md"));

    // ArrowDown at the bottom clamps.
    harness.run(|ctx| {
        for _ in 0..5 {
            panel.handle_event(&press(NamedKey::ArrowDown), ctx);
        }
    });
    assert_eq!(panel.selected_path(), Some("README.md"));

    harness.run(|ctx| {
        panel.handle_event(&press(NamedKey::ArrowUp), ctx);
    });
    assert_eq!(panel.selected_path(), Some("src/main.rs"));

    // ArrowUp at the top clamps.
    harness.run(|ctx| {
        for _ in 0..10 {
            panel.handle_event(&press(NamedKey::ArrowUp), ctx);
        }
    });
    assert_eq!(panel.selected_path(), Some("src/lib.rs"));
}

#[test]
fn escape_hides() {
    let harness = Harness::new();
    let mut panel = GitDiff::new();
    panel.set_files(sample_files());
    panel.show();
    assert!(panel.is_visible());

    harness.run(|ctx| {
        panel.handle_event(&press(NamedKey::Escape), ctx);
    });
    assert!(!panel.is_visible());
}

#[test]
fn service_reply_loads_files() {
    // Native-style: GitService returns Pending(42) so the panel
    // records the id, then a ServiceReply with the matching id and a
    // JSON-serialized `Vec<DiffFile>` lands and the panel decodes it.
    let harness = Harness::with_pending(42);
    let mut panel = GitDiff::new();
    panel.show();

    harness.run(|ctx| {
        panel.refresh(ctx);
    });
    assert!(panel.is_pending());
    assert_eq!(panel.pending_request(), Some(42));

    let payload = serde_json::to_value(sample_files()).unwrap();
    harness.run(|ctx| {
        panel.handle_event(
            &UiEvent::ServiceReply {
                request_id: 42,
                payload,
            },
            ctx,
        );
    });

    assert!(!panel.is_pending());
    assert_eq!(panel.files().len(), 3);
    assert_eq!(panel.selected_path(), Some("src/lib.rs"));
}

#[test]
fn service_reply_with_mismatched_id_is_ignored() {
    let harness = Harness::with_pending(7);
    let mut panel = GitDiff::new();
    panel.show();
    harness.run(|ctx| {
        panel.refresh(ctx);
    });
    assert!(panel.is_pending());

    let payload = serde_json::to_value(sample_files()).unwrap();
    harness.run(|ctx| {
        panel.handle_event(
            &UiEvent::ServiceReply {
                request_id: 99,
                payload,
            },
            ctx,
        );
    });
    // Still pending; files not populated.
    assert!(panel.is_pending());
    assert_eq!(panel.files().len(), 0);
}

#[test]
fn set_files_clamps_selection() {
    let harness = Harness::new();
    let mut panel = GitDiff::new();
    panel.set_files(sample_files());
    panel.show();

    harness.run(|ctx| {
        // Move to the last file.
        panel.handle_event(&press(NamedKey::ArrowDown), ctx);
        panel.handle_event(&press(NamedKey::ArrowDown), ctx);
    });
    assert_eq!(panel.selected_index(), Some(2));

    // Replace the list with a single file — selection must clamp.
    panel.set_files(vec![DiffFile {
        path: "only.txt".to_string(),
        hunks: Vec::new(),
        added: 0,
        removed: 0,
    }]);
    assert_eq!(panel.selected_index(), Some(0));
    assert_eq!(panel.selected_path(), Some("only.txt"));
}

#[test]
fn empty_file_list_has_no_selection() {
    let mut panel = GitDiff::new();
    panel.set_files(Vec::new());
    panel.show();
    assert!(panel.selected_index().is_none());
    assert!(panel.selected_file().is_none());
}

#[test]
fn diff_file_new_sums_added_removed() {
    let file = DiffFile::new(
        "x.rs",
        vec![DiffHunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                DiffLine::Context("ctx".into()),
                DiffLine::Added("a".into()),
                DiffLine::Added("b".into()),
                DiffLine::Removed("r".into()),
            ],
        }],
    );
    assert_eq!(file.added, 2);
    assert_eq!(file.removed, 1);
}

#[test]
fn wants_focus_tracks_visibility() {
    let mut panel = GitDiff::new();
    assert!(!panel.wants_focus());
    panel.show();
    assert!(panel.wants_focus());
    panel.hide();
    assert!(!panel.wants_focus());
}

#[test]
fn name_is_stable_identifier() {
    let panel = GitDiff::new();
    assert_eq!(panel.name(), "git_diff");
}

#[test]
fn diff_line_text_accessor() {
    assert_eq!(DiffLine::Context("c".into()).text(), "c");
    assert_eq!(DiffLine::Added("a".into()).text(), "a");
    assert_eq!(DiffLine::Removed("r".into()).text(), "r");
}
