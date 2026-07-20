//! Sanity tests for the lifted `BufferTabs` panel.
//!
//! Covers:
//! - Construction + the empty-state invariants.
//! - `is_markdown_path` routing through `open_path` -> `open_markdown`.
//! - `move_active` reorder without wrapping.
//! - `agent_for_route` returning the agent kind set via the
//!   `AgentLabel`-bounded `set_terminal_agent` helper.
//! - Click → activate via `hit_test`.
//! - The `Panel` trait surface: focus, resize-clears-layout, and a
//!   pointer-down event activating the correct tab.

use std::path::{Path, PathBuf};

use neoism_ui::layout::{PanelLayout, Rect};
use neoism_ui::panels::buffer_tabs::{
    apply_buffer_tab_policy, AgentLabel, BufferTab, BufferTabPolicyInput,
    BufferTabPolicyOperation, BufferTabs, TabHit,
};
use neoism_ui::panels::{Panel, PanelContext};
use neoism_ui::services::{
    ClipboardService, ClockService, CommandError, CommandService, DirEntry, FilesService,
    GitService, GitStatus, IoError, Services,
};
use neoism_ui::theme::ChromeTheme;
use neoism_ui::{Modifiers, PointerButton, UiEvent};

/// Test-only mirror of the native `AgentKind` so we can exercise the
/// generic surface end to end without depending on `frontends/neoism`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum TestAgent {
    Claude,
    Codex,
}

impl AgentLabel for TestAgent {
    fn display_name(&self) -> &str {
        match self {
            TestAgent::Claude => "Claude Code",
            TestAgent::Codex => "Codex",
        }
    }
}

#[test]
fn neoism_agent_tab_does_not_show_breadcrumbs() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.open_neoism_agent(42);
    assert!(!tabs.active_shows_breadcrumbs());
}

#[test]
fn file_tab_shows_breadcrumbs() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.open_path(PathBuf::from("/tmp/work/src/main.rs"));
    assert!(tabs.active_shows_breadcrumbs());
}

#[test]
fn empty_state_invariants() {
    let tabs: BufferTabs<TestAgent> = BufferTabs::new();
    assert!(!tabs.is_visible());
    assert_eq!(tabs.active(), 0);
    assert!(tabs.tabs().is_empty());
    assert!(!tabs.has_file_tabs());
    assert!(!tabs.active_shows_breadcrumbs());
}

#[test]
fn open_path_for_markdown_routes_to_open_markdown() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    let ix = tabs.open_path(PathBuf::from("/tmp/work/notes.md"));
    assert_eq!(ix, 0);
    assert!(
        tabs.tabs()[0].markdown,
        "open_path on a .md must mark markdown"
    );

    // Re-opening the same path activates instead of duplicating.
    tabs.open_path(PathBuf::from("/tmp/work/src/main.rs"));
    assert_eq!(tabs.tabs().len(), 2);
    let again = tabs.open_path(PathBuf::from("/tmp/work/notes.md"));
    assert_eq!(again, 0);
    assert_eq!(tabs.tabs().len(), 2);
}

#[test]
fn move_active_reorders_current_tab_without_wrapping() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.ensure_terminal_tab();
    tabs.open_path(PathBuf::from("/tmp/work/src/main.rs"));
    tabs.open_path(PathBuf::from("/tmp/work/src/lib.rs"));

    assert_eq!(tabs.active(), 2);
    assert!(tabs.move_active(true));
    assert_eq!(tabs.active(), 1);
    assert_eq!(
        tabs.tabs()[1].path.as_deref(),
        Some(Path::new("/tmp/work/src/lib.rs"))
    );
    assert!(tabs.move_active(true));
    assert_eq!(tabs.active(), 0);
    assert!(!tabs.move_active(true));
}

#[test]
fn shared_policy_cycles_selection_and_selects_by_index() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.ensure_terminal_tab();
    tabs.open_path(PathBuf::from("/tmp/a.rs"));
    tabs.open_path(PathBuf::from("/tmp/b.rs"));

    assert_eq!(tabs.active(), 2);
    assert!(tabs.select_relative(false));
    assert_eq!(tabs.active(), 0, "next selection wraps at the end");
    assert!(tabs.select_relative(true));
    assert_eq!(tabs.active(), 2, "previous selection wraps at the start");
    assert!(tabs.select_index(1));
    assert_eq!(tabs.active(), 1);
    assert!(!tabs.select_index(99));
    assert_eq!(tabs.active(), 1);
}

#[test]
fn shared_policy_reports_move_and_close_bookkeeping() {
    let move_result = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 3,
            active: 1,
            closeable: vec![false, true, true],
        },
        BufferTabPolicyOperation::MoveNext,
    );
    assert!(move_result.changed);
    assert_eq!(move_result.active, 2);
    assert_eq!(move_result.move_from, Some(1));
    assert_eq!(move_result.move_to, Some(2));

    let close_result = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 3,
            active: 2,
            closeable: vec![false, true, true],
        },
        BufferTabPolicyOperation::CloseActive,
    );
    assert!(close_result.changed);
    assert_eq!(close_result.remove_index, Some(2));
    assert_eq!(close_result.active, 1);

    let protected = apply_buffer_tab_policy(
        BufferTabPolicyInput {
            len: 2,
            active: 0,
            closeable: vec![false, true],
        },
        BufferTabPolicyOperation::CloseActive,
    );
    assert!(!protected.changed);
    assert_eq!(protected.remove_index, None);
    assert_eq!(protected.active, 0);
}

#[test]
fn agent_for_route_returns_set_agent() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.open_terminal(7);
    assert!(tabs.set_terminal_agent(7, TestAgent::Codex));
    assert_eq!(tabs.agent_for_route(7), Some(TestAgent::Codex));
    assert_eq!(tabs.tabs()[0].title, "Codex");

    // Re-binding to a different agent keeps the route stable and
    // refreshes the displayed title.
    tabs.open_terminal(8);
    assert!(tabs.set_terminal_agent(8, TestAgent::Claude));
    assert_eq!(tabs.agent_for_route(8), Some(TestAgent::Claude));
}

#[test]
fn hit_test_resolves_activate_inside_the_strip() {
    // Strip width fits one full-width tab (>= MAX_TAB_WIDTH), so each
    // tab is 220 px wide.
    let strip_width = 660.0_f32;
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.open_path(PathBuf::from("/tmp/a.rs"));
    tabs.open_path(PathBuf::from("/tmp/b.rs"));
    tabs.open_path(PathBuf::from("/tmp/c.rs"));

    // Pointer in the middle of the second slot must hit tab #1.
    let hit = tabs.hit_test(330.0, 14.0, 0.0, 0.0, strip_width);
    assert_eq!(hit, Some(TabHit::Activate(1)));
}

// ── Panel trait coverage ──────────────────────────────────────────

/// Tiny no-op services bundle so `PanelContext` can be constructed in
/// tests without touching real filesystems / git / clipboards.
struct NoopFiles;
impl FilesService for NoopFiles {
    fn list_dir(&self, _: &Path) -> Result<Vec<DirEntry>, IoError> {
        Ok(Vec::new())
    }
    fn read_file(&self, _: &Path) -> Result<Vec<u8>, IoError> {
        Ok(Vec::new())
    }
    fn write_file(&self, _: &Path, _: &[u8]) -> Result<(), IoError> {
        Ok(())
    }
    fn stat(&self, _: &Path) -> Result<DirEntry, IoError> {
        Err(IoError::NotFound("noop".into()))
    }
}
struct NoopClipboard;
impl ClipboardService for NoopClipboard {
    fn read(&self) -> Option<String> {
        None
    }
    fn write(&self, _: &str) {}
}
struct NoopCommands;
impl CommandService for NoopCommands {
    fn run(&self, _: &str) -> Result<(), CommandError> {
        Ok(())
    }
}
struct NoopGit;
impl GitService for NoopGit {
    fn status(&self, _: &Path) -> Result<GitStatus, IoError> {
        Ok(GitStatus {
            branch: None,
            dirty: false,
        })
    }
    fn diff(&self, _: &Path, _: Option<&Path>) -> Result<String, IoError> {
        Ok(String::new())
    }
}
struct ZeroClock;
impl ClockService for ZeroClock {
    fn now_monotonic(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
}

fn with_ctx<R>(f: impl FnOnce(&mut PanelContext) -> R) -> R {
    let files = NoopFiles;
    let clipboard = NoopClipboard;
    let commands = NoopCommands;
    let git = NoopGit;
    let clock = ZeroClock;
    let theme = ChromeTheme::default();
    let mut ctx = PanelContext {
        services: Services {
            files: &files,
            clipboard: &clipboard,
            commands: &commands,
            git: &git,
            clock: &clock,
            search: &neoism_ui::services::NullSearchService,
            notifications: &neoism_ui::services::NullNotificationService,
        },
        theme: &theme,
        time: std::time::Duration::ZERO,
    };
    f(&mut ctx)
}

#[test]
fn panel_trait_name_and_focus() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    assert_eq!(tabs.name(), "buffer_tabs");
    assert!(!tabs.wants_focus());

    tabs.open_path(PathBuf::from("/tmp/a.rs"));
    with_ctx(|ctx| {
        tabs.handle_event(&UiEvent::Focus(true), ctx);
    });
    assert!(tabs.wants_focus(), "focus event must take effect");
}

#[test]
fn panel_resize_clears_layout_cache() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.open_path(PathBuf::from("/tmp/a.rs"));
    tabs.layout.push((0.0, 220.0));
    assert!(!tabs.layout.is_empty());

    with_ctx(|ctx| {
        tabs.handle_event(
            &UiEvent::Resize {
                w: 1280,
                h: 800,
                scale: 1.0,
            },
            ctx,
        );
    });
    assert!(
        tabs.layout.is_empty(),
        "Resize must drop the stale layout cache so the next hit_test \
         doesn't trip on stale slot widths"
    );
}

#[test]
fn panel_pointer_down_activates_tab() {
    let mut tabs: BufferTabs<TestAgent> = BufferTabs::new();
    tabs.open_path(PathBuf::from("/tmp/a.rs"));
    tabs.open_path(PathBuf::from("/tmp/b.rs"));
    tabs.open_path(PathBuf::from("/tmp/c.rs"));
    assert_eq!(tabs.active(), 2);

    // Seed the layout cache so `last_strip_width` sees a real strip.
    // Three slots × 220 px each.
    tabs.layout = vec![(0.0, 220.0), (220.0, 220.0), (440.0, 220.0)];

    with_ctx(|ctx| {
        tabs.handle_event(
            &UiEvent::PointerDown {
                button: PointerButton::Left,
                x: 110.0, // middle of first slot
                y: 14.0,
                modifiers: Modifiers::empty(),
                click_count: 1,
            },
            ctx,
        );
    });
    assert_eq!(tabs.active(), 0, "PointerDown on slot 0 must activate it");
}

#[test]
fn buffer_tab_default_layout() {
    // Layout placeholder so PanelLayout still type-checks alongside
    // the Panel trait's `draw` signature even though we don't paint.
    let _layout = PanelLayout {
        bounds: Rect::new(0.0, 0.0, 660.0, 28.0),
        scale: 1.0,
    };
    let _tab: BufferTab<TestAgent> = BufferTab {
        title: "x".into(),
        modified: false,
        path: None,
        markdown: false,
        terminal_route_id: None,
        neoism_agent_route_id: None,
        chrome_page: None,
        agent_kind: None,
    };
}
