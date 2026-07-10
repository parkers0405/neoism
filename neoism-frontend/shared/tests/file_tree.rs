//! Behavior tests for the slim file tree panel.
//!
//! Covers:
//! - `apply_listing` populates the flat node list under the root,
//!   dirs sorted before files, both alphabetical case-insensitive.
//! - Arrow keys move the selection cursor with clamping.
//! - `open_dir` against a pending `FilesService` parks the request
//!   in `pending`; the subsequent `UiEvent::ServiceReply` with the
//!   matching request id applies the listing under the right path.
//! - Initial population never performs Git I/O; the explicit refresh
//!   request does it later and rejects results for a superseded root.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use neoism_ui::event::{
    KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PhysicalKey, UiEvent,
};
use neoism_ui::panels::file_tree::{GitStatus as TreeGitStatus, NodeKind};
use neoism_ui::panels::{FileTree, Panel, PanelContext};
use neoism_ui::services::{
    ClipboardService, ClockService, CommandError, CommandService, DirEntry, FilesService,
    GitService, GitStatus, IoError, RequestId, Services,
};
use neoism_ui::theme::ChromeTheme;

// -- Service stubs ----------------------------------------------------------

/// `FilesService` that always returns `IoError::Pending` with a
/// fixed request id. Records every `list_dir` call so the test can
/// assert the parked path.
struct PendingFiles {
    req_id: RequestId,
    calls: Mutex<Vec<PathBuf>>,
}

impl PendingFiles {
    fn new(req_id: RequestId) -> Self {
        Self {
            req_id,
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl FilesService for PendingFiles {
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, IoError> {
        self.calls.lock().unwrap().push(path.to_path_buf());
        Err(IoError::Pending(self.req_id))
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

/// `FilesService` that returns a canned listing synchronously.
struct CannedFiles {
    listing: Vec<DirEntry>,
}

impl FilesService for CannedFiles {
    fn list_dir(&self, _path: &Path) -> Result<Vec<DirEntry>, IoError> {
        Ok(self.listing.clone())
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

struct CountingGit {
    porcelain_calls: AtomicUsize,
}

impl CountingGit {
    fn new() -> Self {
        Self {
            porcelain_calls: AtomicUsize::new(0),
        }
    }
}

impl GitService for CountingGit {
    fn status(&self, _repo: &Path) -> Result<GitStatus, IoError> {
        Ok(GitStatus {
            branch: Some("main".to_string()),
            dirty: true,
        })
    }

    fn diff(&self, _repo: &Path, _path: Option<&Path>) -> Result<String, IoError> {
        Ok(String::new())
    }

    fn status_porcelain(&self, _repo: &Path) -> Result<Vec<u8>, IoError> {
        self.porcelain_calls.fetch_add(1, Ordering::SeqCst);
        Ok(b" M README.md\0".to_vec())
    }

    fn repo_root(&self, cwd: &Path) -> Option<PathBuf> {
        Some(cwd.to_path_buf())
    }
}

struct FixedClock;
impl ClockService for FixedClock {
    fn now_monotonic(&self) -> Duration {
        Duration::from_millis(0)
    }
}

/// Holds services so a single closure can borrow them through
/// `PanelContext`. `RefCell` available to extend the harness; not
/// currently needed.
struct Harness<F: FilesService> {
    files: F,
    clipboard: NullClipboard,
    commands: NullCommands,
    git: NullGit,
    clock: FixedClock,
    theme: ChromeTheme,
}

impl<F: FilesService> Harness<F> {
    fn new(files: F) -> Self {
        Self {
            files,
            clipboard: NullClipboard,
            commands: NullCommands,
            git: NullGit,
            clock: FixedClock,
            theme: ChromeTheme::default(),
        }
    }

    fn run<R>(&self, f: impl FnOnce(&mut PanelContext) -> R) -> R {
        self.run_with_git(&self.git, f)
    }

    fn run_with_git<R>(
        &self,
        git: &dyn GitService,
        f: impl FnOnce(&mut PanelContext) -> R,
    ) -> R {
        let services = Services {
            files: &self.files,
            clipboard: &self.clipboard,
            commands: &self.commands,
            git,
            clock: &self.clock,
            search: &neoism_ui::services::NullSearchService,
            notifications: &neoism_ui::services::NullNotificationService,
        };
        let mut ctx = PanelContext {
            services,
            theme: &self.theme,
            time: Duration::from_millis(0),
        };
        f(&mut ctx)
    }
}

// -- Test helpers -----------------------------------------------------------

fn press(named: NamedKey) -> UiEvent {
    UiEvent::Key(KeyDescriptor {
        physical: PhysicalKey(0),
        logical: LogicalKey::Named(named),
        state: KeyState::Pressed,
        modifiers: Modifiers::empty(),
        repeat: false,
    })
}

fn entry(name: &str, is_dir: bool) -> DirEntry {
    DirEntry {
        name: name.to_string(),
        is_dir,
        size: None,
    }
}

fn is_dir(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Dir { .. })
}

// -- Tests ------------------------------------------------------------------

#[test]
fn apply_listing_populates_tree() {
    let mut tree = FileTree::new(PathBuf::from("/workspace"));
    let listing = vec![entry("src", true), entry("README.md", false)];
    tree.apply_listing(Path::new("/workspace"), listing);

    let nodes = tree.nodes();
    assert_eq!(nodes.len(), 2, "two top-level rows expected");
    // Dirs sort before files.
    assert_eq!(nodes[0].label, "src");
    assert!(is_dir(&nodes[0].kind));
    assert_eq!(nodes[0].depth, 0);
    assert_eq!(nodes[0].path.as_deref(), Some(Path::new("/workspace/src")));

    assert_eq!(nodes[1].label, "README.md");
    assert!(!is_dir(&nodes[1].kind));
    assert_eq!(nodes[1].depth, 0);
    assert_eq!(
        nodes[1].path.as_deref(),
        Some(Path::new("/workspace/README.md"))
    );
}

#[test]
fn arrow_keys_move_selection() {
    let mut tree = FileTree::new(PathBuf::from("/workspace"));
    tree.apply_listing(
        Path::new("/workspace"),
        vec![entry("a", false), entry("b", false), entry("c", false)],
    );
    assert_eq!(tree.selected_path(), Some(Path::new("/workspace/a")));
    tree.set_focused(true);

    // ArrowUp at top is a no-op clamp.
    let harness = Harness::new(CannedFiles { listing: vec![] });
    harness.run(|ctx| {
        tree.handle_event(&press(NamedKey::ArrowUp), ctx);
    });
    assert_eq!(tree.selected_path(), Some(Path::new("/workspace/a")));

    // Two ArrowDowns move to the third row.
    harness.run(|ctx| {
        tree.handle_event(&press(NamedKey::ArrowDown), ctx);
        tree.handle_event(&press(NamedKey::ArrowDown), ctx);
    });
    assert_eq!(tree.selected_path(), Some(Path::new("/workspace/c")));

    // Pile-on Downs clamp at the last row.
    harness.run(|ctx| {
        for _ in 0..10 {
            tree.handle_event(&press(NamedKey::ArrowDown), ctx);
        }
    });
    assert_eq!(tree.selected_path(), Some(Path::new("/workspace/c")));
}

#[test]
fn service_reply_resolves_pending_listing() {
    let mut tree = FileTree::new(PathBuf::from("/workspace"));
    // Seed the root so the dir we open is a known node.
    tree.apply_listing(
        Path::new("/workspace"),
        vec![entry("src", true), entry("README.md", false)],
    );

    let harness = Harness::new(PendingFiles::new(7));
    let target = PathBuf::from("/workspace/src");

    // Open the dir. `list_dir` returns Pending(7) — pending map
    // records `(7 -> /workspace/src)`. No nodes change yet.
    harness.run(|ctx| {
        tree.open_dir(&target, ctx);
    });
    assert_eq!(
        tree.pending_len(),
        1,
        "open_dir should park the request when the service returns Pending"
    );
    assert!(tree.is_expanded(&target));
    assert_eq!(
        harness.files.calls.lock().unwrap().as_slice(),
        &[target.clone()],
        "the service was called exactly once with the target path"
    );
    assert_eq!(tree.nodes().len(), 2, "no children spliced in yet");

    // The daemon delivers the reply. Reuse a Vec<DirEntry> JSON
    // payload to match the format the host wraps replies in.
    let reply_entries = vec![entry("lib.rs", false), entry("main.rs", false)];
    let payload = serde_json::to_value(&reply_entries).unwrap();
    harness.run(|ctx| {
        tree.handle_event(
            &UiEvent::ServiceReply {
                request_id: 7,
                payload,
            },
            ctx,
        );
    });

    assert_eq!(tree.pending_len(), 0, "reply should pop the pending entry");
    let nodes = tree.nodes();
    assert_eq!(
        nodes.len(),
        4,
        "two children spliced in after /workspace/src"
    );
    assert_eq!(nodes[0].label, "src");
    assert_eq!(nodes[0].depth, 0);
    assert_eq!(nodes[1].label, "lib.rs");
    assert_eq!(nodes[1].depth, 1);
    assert_eq!(
        nodes[1].path.as_deref(),
        Some(Path::new("/workspace/src/lib.rs"))
    );
    assert_eq!(nodes[2].label, "main.rs");
    assert_eq!(nodes[2].depth, 1);
    assert_eq!(nodes[3].label, "README.md");
    assert_eq!(nodes[3].depth, 0);
}

#[test]
fn unknown_service_reply_id_is_ignored() {
    let mut tree = FileTree::new(PathBuf::from("/workspace"));
    tree.apply_listing(Path::new("/workspace"), vec![entry("a.txt", false)]);
    let harness = Harness::new(CannedFiles { listing: vec![] });
    let payload = serde_json::to_value::<Vec<DirEntry>>(vec![]).unwrap();
    harness.run(|ctx| {
        tree.handle_event(
            &UiEvent::ServiceReply {
                request_id: 999,
                payload,
            },
            ctx,
        );
    });
    // Nothing changes — unknown request id is dropped silently.
    assert_eq!(tree.nodes().len(), 1);
}

#[test]
fn enter_on_dir_uses_synchronous_listing() {
    let mut tree = FileTree::new(PathBuf::from("/workspace"));
    tree.apply_listing(Path::new("/workspace"), vec![entry("src", true)]);

    let harness = Harness::new(CannedFiles {
        listing: vec![entry("inner.rs", false)],
    });
    tree.set_focused(true);
    harness.run(|ctx| {
        // Enter on the selected dir row should expand it via the
        // sync service path (no Pending), so children appear
        // immediately and `pending` stays empty.
        tree.handle_event(&press(NamedKey::Enter), ctx);
    });

    assert_eq!(tree.pending_len(), 0);
    assert!(tree.is_expanded(Path::new("/workspace/src")));
    let nodes = tree.nodes();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[1].label, "inner.rs");
    assert_eq!(nodes[1].depth, 1);
}

#[test]
fn close_dir_removes_descendants() {
    let mut tree = FileTree::new(PathBuf::from("/workspace"));
    tree.apply_listing(
        Path::new("/workspace"),
        vec![entry("src", true), entry("after.txt", false)],
    );
    let harness = Harness::new(CannedFiles {
        listing: vec![entry("a.rs", false), entry("b.rs", false)],
    });
    harness.run(|ctx| {
        tree.open_dir(Path::new("/workspace/src"), ctx);
    });
    assert_eq!(tree.nodes().len(), 4);

    tree.close_dir(Path::new("/workspace/src"));
    assert_eq!(tree.nodes().len(), 2, "children should be dropped");
    assert!(!tree.is_expanded(Path::new("/workspace/src")));
}

#[test]
fn name_and_focus_defaults() {
    let tree = FileTree::new(PathBuf::from("/workspace"));
    assert_eq!(tree.name(), "file_tree");
    assert!(!tree.wants_focus());
}

#[test]
fn initial_population_defers_git_status_to_explicit_refresh() {
    let root = PathBuf::from("/workspace");
    let mut tree = FileTree::empty();
    let git = CountingGit::new();
    let harness = Harness::new(CannedFiles {
        listing: vec![entry("README.md", false)],
    });

    harness.run_with_git(&git, |ctx| tree.populate_from_dir(&root, ctx));
    assert_eq!(
        git.porcelain_calls.load(Ordering::SeqCst),
        0,
        "opening the tree must not run git status on the UI thread"
    );
    assert_eq!(tree.entries()[0].git_status, TreeGitStatus::None);

    let request = tree.git_refresh_request().unwrap();
    let result =
        harness.run_with_git(&git, |ctx| FileTree::run_git_refresh_request(request, ctx));
    assert_eq!(git.porcelain_calls.load(Ordering::SeqCst), 1);
    assert!(tree.apply_git_refresh_result(result));
    assert_eq!(tree.entries()[0].git_status, TreeGitStatus::Modified);
}

#[test]
fn stale_git_refresh_result_cannot_cross_workspace_roots() {
    let first_root = PathBuf::from("/workspace-a");
    let second_root = PathBuf::from("/workspace-b");
    let mut tree = FileTree::empty();
    let git = CountingGit::new();
    let harness = Harness::new(CannedFiles {
        listing: vec![entry("README.md", false)],
    });

    harness.run_with_git(&git, |ctx| tree.populate_from_dir(&first_root, ctx));
    let request = tree.git_refresh_request().unwrap();
    let stale =
        harness.run_with_git(&git, |ctx| FileTree::run_git_refresh_request(request, ctx));

    harness.run_with_git(&git, |ctx| tree.populate_from_dir(&second_root, ctx));
    assert!(!tree.apply_git_refresh_result(stale));
    assert_eq!(tree.root(), Some(second_root.as_path()));
    assert_eq!(tree.entries()[0].git_status, TreeGitStatus::None);
}

// Held to keep `RefCell` in scope for future harness extensions.
#[allow(dead_code)]
fn _hold_refcell() -> RefCell<u8> {
    RefCell::new(0)
}
