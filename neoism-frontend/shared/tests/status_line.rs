//! Sanity tests for the migrated status line panel.
//!
//! `render()` and `draw()` both need a live `sugarloaf::Sugarloaf` to
//! exercise — they're left for an integration suite that wires the
//! renderer up. The unit checks here cover state mutation, hit-test
//! identity, and the `Panel` event surface.

use std::time::Duration;

use neoism_ui::event::{Modifiers, PointerButton, ThemeChange, UiEvent};
use neoism_ui::layout::{PanelLayout, Rect};
use neoism_ui::panels::status_line::{
    DiagnosticCounts, DiagnosticPill, GitChangeSummary, LspStatus, Mode, PrimaryKind,
    StatusInfo, StatusLine,
};
use neoism_ui::panels::{Panel, PanelContext};
use neoism_ui::services::{
    ClipboardService, ClockService, CommandError, CommandService, DirEntry, FilesService,
    GitService, GitStatus, IoError, Services,
};
use neoism_ui::theme::ChromeTheme;

// ── stub services ───────────────────────────────────────────────────

struct StubFiles;
impl FilesService for StubFiles {
    fn list_dir(&self, _: &std::path::Path) -> Result<Vec<DirEntry>, IoError> {
        Ok(vec![])
    }
    fn read_file(&self, _: &std::path::Path) -> Result<Vec<u8>, IoError> {
        Ok(vec![])
    }
    fn write_file(&self, _: &std::path::Path, _: &[u8]) -> Result<(), IoError> {
        Ok(())
    }
    fn stat(&self, _: &std::path::Path) -> Result<DirEntry, IoError> {
        Err(IoError::NotFound("stub".into()))
    }
}

struct StubClipboard;
impl ClipboardService for StubClipboard {
    fn read(&self) -> Option<String> {
        None
    }
    fn write(&self, _: &str) {}
}

struct StubCommands;
impl CommandService for StubCommands {
    fn run(&self, _: &str) -> Result<(), CommandError> {
        Ok(())
    }
}

struct StubGit;
impl GitService for StubGit {
    fn status(&self, _: &std::path::Path) -> Result<GitStatus, IoError> {
        Ok(GitStatus {
            branch: None,
            dirty: false,
        })
    }
    fn diff(
        &self,
        _: &std::path::Path,
        _: Option<&std::path::Path>,
    ) -> Result<String, IoError> {
        Ok(String::new())
    }
}

struct StubClock;
impl ClockService for StubClock {
    fn now_monotonic(&self) -> Duration {
        Duration::ZERO
    }
}

fn with_ctx(theme: &ChromeTheme, f: impl FnOnce(&mut PanelContext)) {
    let files = StubFiles;
    let clipboard = StubClipboard;
    let commands = StubCommands;
    let git = StubGit;
    let clock = StubClock;
    let services = Services {
        files: &files,
        clipboard: &clipboard,
        commands: &commands,
        git: &git,
        clock: &clock,
        search: &neoism_ui::services::NullSearchService,
        notifications: &neoism_ui::services::NullNotificationService,
    };
    let mut ctx = PanelContext {
        services,
        theme,
        time: Duration::ZERO,
    };
    f(&mut ctx);
}

// ── tests ───────────────────────────────────────────────────────────

#[test]
fn status_line_constructs() {
    let mut s = StatusLine::new();
    assert_eq!(s.info().mode, Mode::Normal);
    assert!(s.info().branch.is_none());
    assert_eq!(s.scale(), 1.0);

    let info = StatusInfo {
        mode: Mode::Insert,
        primary: "src/main.rs".into(),
        primary_kind: PrimaryKind::File,
        branch: Some("main".into()),
        git_changes: Some(GitChangeSummary {
            added: 3,
            deleted: 1,
        }),
        workspace: Some("ws".into()),
        lsp_status: Some(LspStatus::Active),
        lsp_label: None,
        project: Some("neoism".into()),
        cursor_lines: Some((1, 4)),
        diagnostics: DiagnosticCounts {
            error: 2,
            warn: 1,
            info: 0,
            hint: 0,
        },
        cwd_label: Some("~/proj".into()),
        pending_keys: None,
        fps: Some(120),
    };
    s.set_info(info);

    let snap = s.info();
    assert_eq!(snap.mode, Mode::Insert);
    assert_eq!(snap.primary, "src/main.rs");
    assert_eq!(snap.branch.as_deref(), Some("main"));
    assert_eq!(snap.git_changes.unwrap().added, 3);
    assert_eq!(snap.diagnostics.error, 2);
    assert_eq!(snap.cwd_label.as_deref(), Some("~/proj"));

    // Mode flip arms the cross-fade animation.
    assert!(s.is_animating());
}

#[test]
fn status_line_ignores_unrelated_events() {
    let mut s = StatusLine::new();
    s.set_info(StatusInfo {
        mode: Mode::Normal,
        branch: Some("trunk".into()),
        cwd_label: Some("~/x".into()),
        diagnostics: DiagnosticCounts {
            error: 1,
            warn: 0,
            info: 0,
            hint: 0,
        },
        ..StatusInfo::default()
    });
    // Snapshot info BEFORE handing the panel an event — we only need
    // to confirm `handle_event` doesn't mutate the snapshot at all.
    let before_mode = s.info().mode;
    let before_branch = s.info().branch.clone();
    let before_diag = s.info().diagnostics;

    let theme = ChromeTheme::default();
    with_ctx(&theme, |ctx| {
        s.handle_event(
            &UiEvent::PointerDown {
                button: PointerButton::Left,
                x: 100.0,
                y: 100.0,
                modifiers: Modifiers::empty(),
                click_count: 1,
            },
            ctx,
        );
        s.handle_event(
            &UiEvent::Theme(ThemeChange {
                palette_dirty: true,
                scale_changed: None,
            }),
            ctx,
        );
        s.handle_event(&UiEvent::Tick(Duration::from_millis(16)), ctx);
    });

    assert_eq!(s.info().mode, before_mode);
    assert_eq!(s.info().branch, before_branch);
    assert_eq!(s.info().diagnostics, before_diag);
}

#[test]
fn status_line_handles_resize() {
    let mut s = StatusLine::new();
    let theme = ChromeTheme::default();
    with_ctx(&theme, |ctx| {
        s.handle_event(
            &UiEvent::Resize {
                w: 1920,
                h: 1080,
                scale: 1.0,
            },
            ctx,
        );
        s.handle_event(
            &UiEvent::Resize {
                w: 3840,
                h: 2160,
                scale: 2.0,
            },
            ctx,
        );
    });
    // No state for resize to mutate — the strip is re-laid-out per
    // paint. The test passes if neither call panics.
}

#[test]
fn status_line_scale_clamps() {
    let mut s = StatusLine::new();
    s.set_scale(0.1); // below SCALE_MIN (0.5)
    assert!(s.scale() >= 0.5);
    s.set_scale(10.0); // above SCALE_MAX (3.0)
    assert!(s.scale() <= 3.0);
    s.set_scale(1.25);
    assert_eq!(s.scale(), 1.25);
    assert!((s.scaled_height() - 22.0 * 1.25).abs() < 1e-5);
}

#[test]
fn status_line_diagnostic_hit_test_empty() {
    let s = StatusLine::new();
    // Without ever rendering, every hit-rect is the default zero rect.
    assert_eq!(s.diagnostic_pill_at(10.0, 10.0), None);
    assert!(!s.git_branch_at(10.0, 10.0));
    assert!(!s.split_toggle_at(10.0, 10.0));
}

#[test]
fn status_line_split_toggle_enable_disable() {
    let mut s = StatusLine::new();
    s.set_split_toggle(true, false);
    // Even with the toggle enabled, hit-test stays false until a
    // render populates `split_toggle_rect`.
    assert!(!s.split_toggle_at(10.0, 10.0));
    s.set_split_toggle(false, false);
    assert!(!s.split_toggle_at(10.0, 10.0));
}

#[test]
fn status_line_branch_hover_toggles_when_branch_present() {
    let mut s = StatusLine::new();
    // No branch: hover request is rejected (returns false).
    assert!(!s.set_git_branch_hovered(true));
    assert!(!s.git_branch_hovered());

    s.set_info(StatusInfo {
        branch: Some("main".into()),
        ..StatusInfo::default()
    });
    // With a branch present, the hover state can flip.
    assert!(s.set_git_branch_hovered(true));
    assert!(s.git_branch_hovered());
    // Setting the same state again is a no-op (returns false).
    assert!(!s.set_git_branch_hovered(true));
}

#[test]
fn status_line_diagnostic_pill_anchor_none_without_render() {
    let s = StatusLine::new();
    assert!(s.diagnostic_pill_anchor(DiagnosticPill::Error).is_none());
    assert!(s.diagnostic_pill_anchor(DiagnosticPill::Warn).is_none());
}

#[test]
fn status_line_panel_name() {
    let s = StatusLine::new();
    assert_eq!(s.name(), "status_line");
    // `wants_focus` defaults to false — the strip never holds the
    // keyboard.
    assert!(!s.wants_focus());
}

#[test]
fn status_line_visibility() {
    let mut s = StatusLine::new();
    assert!(s.is_visible());
    s.set_visible(false);
    assert!(!s.is_visible());
    s.set_visible(true);
    assert!(s.is_visible());
}

#[test]
fn status_line_panel_draw_is_noop_without_palette() {
    // The trait `draw` is intentionally a no-op until `ChromeTheme`
    // grows enough channels to drive the strip. The test confirms
    // that calling it via the trait doesn't blow up — even though we
    // pass a dummy `&mut Sugarloaf` substitute would require a real
    // graphics context, the no-op impl skips touching it.
    //
    // We can't construct a `Sugarloaf` headless, so this test only
    // covers that the trait method is callable as a `&dyn Panel`.
    let s = StatusLine::new();
    let panel: &dyn Panel = &s;
    assert_eq!(panel.name(), "status_line");
    assert!(!panel.wants_focus());

    let _layout = PanelLayout {
        bounds: Rect::new(0.0, 0.0, 800.0, 22.0),
        scale: 1.0,
    };
    // Not actually calling draw — it needs a live Sugarloaf. Layout
    // construction alone exercises the rect/scale interface.
}
