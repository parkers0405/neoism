//! Host-neutral markdown bridge policy.
//!
//! Pure helpers extracted from the desktop `Screen` markdown bridge. The
//! host still owns native event adaptation, clipboard IO, filesystem
//! writes, and direct `MarkdownPane` mutation — the helpers here only
//! translate POD inputs into POD plans.
//!
//! The big `dispatch_markdown_key` match arms in the desktop fork are
//! intentionally left in place because their bodies call short-circuited
//! mutating methods on `MarkdownPane` (e.g. `move_table_cell || indent_list_item`)
//! where the boolean result decides whether the next branch fires. The
//! pure pieces of that dispatch that *are* extractable — the leader
//! state machine, the modifier-class decomposition, and the
//! command-palette / leader-x special cases — live here.

use std::path::{Path, PathBuf};

use super::types::{
    MarkdownBlockTemplate, MarkdownLinkTarget, MarkdownMode, MarkdownWikiLinkKind,
};

/// Maximum gap (in milliseconds) between the leader key and a follow-up
/// key before the leader chord is dropped.
pub const LEADER_TIMEOUT_MS: u128 = 800;

// ---------------------------------------------------------------------------
// Modifier class
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkdownBridgeModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkdownModifierClass {
    /// No modifier key (other than `Shift`) is held.
    pub plain: bool,
    /// Only the Control key is held (no Alt/Super).
    pub ctrl_only: bool,
    /// Only the Super (Cmd/Win) key is held (no Control/Alt).
    pub super_only: bool,
}

impl MarkdownBridgeModifiers {
    /// Decompose the raw modifier mask into the boolean classes the
    /// dispatcher branches on. Mirrors the historical desktop checks
    /// (`plain` = none of ctrl/alt/super).
    pub fn classify(self) -> MarkdownModifierClass {
        let plain = !self.control && !self.alt && !self.super_key;
        let ctrl_only = self.control && !self.alt && !self.super_key;
        let super_only = self.super_key && !self.control && !self.alt;
        MarkdownModifierClass {
            plain,
            ctrl_only,
            super_only,
        }
    }
}

// ---------------------------------------------------------------------------
// Leader-key state machine
// ---------------------------------------------------------------------------

/// Result of consulting the leader-key state machine on a fresh key
/// press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkdownLeaderTransition {
    /// The host should clear `markdown_leader_pending` (the timer
    /// expired or the chord was consumed/cancelled).
    pub clear_pending: bool,
    /// The leader expired before a follow-up arrived; the host should
    /// treat the prior `<Space>` press as a no-op and fall through to
    /// regular handling.
    pub flushed: bool,
}

/// Decide what to do with a pending leader timer when a new key arrives.
///
/// Inputs are POD (`elapsed_ms` since the leader was armed, plus mode +
/// pending flag). The host applies the returned transition to its
/// `markdown_leader_pending` field.
pub fn markdown_leader_transition(
    mode: Option<MarkdownMode>,
    leader_pending: bool,
    elapsed_ms: u128,
) -> MarkdownLeaderTransition {
    let normal = matches!(mode, Some(MarkdownMode::Normal));
    if !normal {
        return MarkdownLeaderTransition {
            clear_pending: leader_pending,
            flushed: false,
        };
    }
    if !leader_pending {
        return MarkdownLeaderTransition::default();
    }
    if elapsed_ms > LEADER_TIMEOUT_MS {
        // Expired: drop the timer, the host should also treat any
        // queued behaviour as flushed (scroll-by-page in dispatcher).
        MarkdownLeaderTransition {
            clear_pending: true,
            flushed: true,
        }
    } else {
        // The next key consumes the chord — drop the timer; whether
        // this key matched a chord binding is decided by the host's
        // dispatcher.
        MarkdownLeaderTransition {
            clear_pending: true,
            flushed: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Link target plan
// ---------------------------------------------------------------------------

/// What the host should do after resolving a `MarkdownLinkTarget`
/// activation. `OpenMarkdownPath` carries the `create_missing_note`
/// flag — file-system writes are host-only, so the host either creates
/// the note or short-circuits per its policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownLinkOpenPlan {
    OpenDirectoryInFileTree {
        path: PathBuf,
    },
    OpenMarkdownPath {
        path: PathBuf,
        line: Option<usize>,
        create_missing_note: bool,
    },
    OpenEditorPath {
        path: PathBuf,
        line: Option<usize>,
    },
}

pub fn markdown_link_open_plan(
    target: &MarkdownLinkTarget,
    path_is_dir: bool,
    path_is_markdown: bool,
    path_exists: bool,
) -> MarkdownLinkOpenPlan {
    let path = target.path.clone();
    if path_is_dir {
        return MarkdownLinkOpenPlan::OpenDirectoryInFileTree { path };
    }
    if path_is_markdown {
        let create_missing_note = !target.code_ref && !path_exists;
        return MarkdownLinkOpenPlan::OpenMarkdownPath {
            path,
            line: target.line,
            create_missing_note,
        };
    }
    MarkdownLinkOpenPlan::OpenEditorPath {
        path,
        line: target.line,
    }
}

// ---------------------------------------------------------------------------
// Missing-note creation policy
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MissingNoteDecision {
    /// Path is outside the active workspace root; host should refuse
    /// and surface the warning message.
    Refuse { warning: String },
    /// Host should create the note file with the provided seed
    /// contents.
    Create { contents: String },
}

/// Decide whether a missing markdown note may be created and, if so,
/// what initial bytes the host should write. The host still does the
/// filesystem I/O (`create_dir_all`, `OpenOptions::create_new`, ...).
pub fn missing_markdown_note_decision(
    path: &Path,
    workspace_root: Option<&Path>,
) -> MissingNoteDecision {
    if let Some(root) = workspace_root {
        if path.is_absolute() && !path.starts_with(root) {
            return MissingNoteDecision::Refuse {
                warning: format!(
                    "Refusing to create note outside workspace: {}",
                    path.display()
                ),
            };
        }
    }

    let title = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.trim())
        .filter(|stem| {
            !(stem.starts_with('.')
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_none())
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or("Untitled");
    MissingNoteDecision::Create {
        contents: format!("# {title}\n\n"),
    }
}

// ---------------------------------------------------------------------------
// Editor jump-to-line lua command
// ---------------------------------------------------------------------------

/// Build the `lua pcall(...)` command the desktop host sends to its
/// nvim editor route to jump the cursor to `line` and centre the
/// viewport. Pure string composition; the lua-string literal escaping
/// is supplied by the caller to avoid coupling to the nvim performer
/// crate.
pub fn markdown_link_editor_jump_command(
    line_one_based: usize,
    path_lit: &str,
) -> String {
    let line = line_one_based.max(1);
    format!(
        "lua pcall(function() vim.cmd.edit({path_lit}); vim.api.nvim_win_set_cursor(0, {{ {line}, 0 }}); vim.cmd('normal! zz'); require('rio.search').preview({line}) end)"
    )
}

// ---------------------------------------------------------------------------
// Tab strip move plan
// ---------------------------------------------------------------------------

/// Where the markdown buffer-tab content currently lives, from the
/// perspective of `move_markdown_tab_between_strips`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownTabSource {
    Workspace,
    Pane(usize),
}

/// Where the markdown buffer-tab content is being moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownTabDestination {
    Workspace,
    Pane(usize),
}

/// What the host should attempt for the destination pane in a markdown
/// tab move. Computed purely from `markdown_route` (the route already
/// hosting this file in the source strip) + the destination strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownTabMovePlan {
    /// Drop the tab onto the workspace strip; if `existing_route` is
    /// `Some`, stack it onto the workspace, else `activate_markdown_path`.
    AttachWorkspace { existing_route: Option<usize> },
    /// Stack the existing markdown route onto `dest_route`.
    StackExistingOnPane {
        markdown_route: usize,
        dest_route: usize,
    },
    /// No existing route — host must create one via
    /// `ensure_pane_markdown_route_for_file`.
    EnsurePaneRoute { dest_route: usize },
}

pub fn markdown_tab_move_plan(
    markdown_route: Option<usize>,
    dest: MarkdownTabDestination,
) -> MarkdownTabMovePlan {
    match dest {
        MarkdownTabDestination::Workspace => MarkdownTabMovePlan::AttachWorkspace {
            existing_route: markdown_route,
        },
        MarkdownTabDestination::Pane(dest_route) => match markdown_route {
            Some(route) => MarkdownTabMovePlan::StackExistingOnPane {
                markdown_route: route,
                dest_route,
            },
            None => MarkdownTabMovePlan::EnsurePaneRoute { dest_route },
        },
    }
}

/// User-facing warning string when a tab move into a split fails.
pub fn markdown_tab_move_failure_message(title: &str) -> String {
    format!("Could not move `{title}` into that split.")
}

/// User-facing warning string when a tab move into a split fails for
/// Neoism agent-bridge tabs (mirrors markdown copy).
pub fn markdown_tab_move_failure_message_for_agent(title: &str) -> String {
    format!("Could not move `{title}` into that split.")
}

/// User-facing warning string when tearing a tab out into a new split
/// fails.
pub fn markdown_tab_tear_out_failure_message(title: &str) -> String {
    format!("Could not tear out `{title}` to a split.")
}

// ---------------------------------------------------------------------------
// apply_markdown_block_template decision
// ---------------------------------------------------------------------------

/// Decide whether applying `template` should re-open the wiki-link
/// completion menu. Pure mirror of the inline `WikiLink | CodeLink`
/// branch that fires inside `Screen::apply_markdown_block_template`.
///
/// The dedicated `markdown_block_template_opens_link_completion`
/// predicate lives in `panels::editor::markdown::menus`; this is a
/// thin re-export so bridge callers can keep all decision lookups in
/// one module.
pub fn apply_markdown_block_template_refreshes_link_completion(
    template: MarkdownBlockTemplate,
) -> bool {
    matches!(
        template,
        MarkdownBlockTemplate::WikiLink | MarkdownBlockTemplate::CodeLink
    )
}

// ---------------------------------------------------------------------------
// refresh_markdown_link_completion_menu decision
// ---------------------------------------------------------------------------

/// What the host should do with the markdown link completion menu
/// after a refresh request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownLinkCompletionDecision {
    /// No active query — host should close the menu if it currently
    /// shows a link completion; otherwise leave everything alone.
    NoQuery,
    /// Query is in line-suffix mode (`@target-123`) which suppresses
    /// the menu — host closes any open completion menu.
    SuppressForLineSuffix,
    /// Host should render the menu using `title` and the supplied
    /// list of (target, meta) pairs. `create_target`, when `Some`, is
    /// the synthetic note-creation target appended at the end of the
    /// list (so the host knows which row triggers note creation).
    Open {
        title: &'static str,
        kind: MarkdownWikiLinkKind,
        create_target: Option<String>,
    },
    /// Query resolved but produced no suggestions — host closes any
    /// existing completion menu.
    NoSuggestions,
}

/// Decide what the host should do with the wiki-link completion menu
/// for a given query. The host is still in charge of computing
/// `suggestions_empty` (it depends on the indexed/uncached fallback
/// scan) and computing the final menu items — this just classifies
/// the high-level transition.
pub fn markdown_link_completion_decision(
    query_kind: MarkdownWikiLinkKind,
    query_text: &str,
    suggestions_empty_before_create: bool,
    create_target: Option<String>,
) -> MarkdownLinkCompletionDecision {
    if matches!(query_kind, MarkdownWikiLinkKind::CodeRef)
        && markdown_link_line_suffix_mode(query_text)
    {
        return MarkdownLinkCompletionDecision::SuppressForLineSuffix;
    }

    if suggestions_empty_before_create && create_target.is_none() {
        return MarkdownLinkCompletionDecision::NoSuggestions;
    }

    let title = match query_kind {
        MarkdownWikiLinkKind::CodeRef => "Link code",
        MarkdownWikiLinkKind::Heading => "Link heading",
        MarkdownWikiLinkKind::Note => "Link note",
    };
    MarkdownLinkCompletionDecision::Open {
        title,
        kind: query_kind,
        create_target,
    }
}

/// `<target>-<line>` line-suffix detector for the CodeRef query — same
/// rule the desktop bridge uses to suppress menu re-opens after the
/// user has typed an explicit `-123` line number. Pure string scan.
pub fn markdown_link_line_suffix_mode(query: &str) -> bool {
    let query = query.trim();
    query.rsplit_once('-').is_some_and(|(target, line)| {
        !target.trim().is_empty() && line.chars().all(|ch| ch.is_ascii_digit())
    })
}

// ---------------------------------------------------------------------------
// dispatch_markdown_key top-level decisions
// ---------------------------------------------------------------------------

/// Top-level guard the dispatcher runs before any mode-specific
/// handling. When `true`, the host should open the command palette and
/// short-circuit the rest of the dispatcher.
pub fn markdown_normal_colon_opens_palette(
    mode: Option<MarkdownMode>,
    classes: MarkdownModifierClass,
    is_colon_char: bool,
) -> bool {
    matches!(mode, Some(MarkdownMode::Normal)) && classes.plain && is_colon_char
}

/// `<Space>x` leader chord: when armed and the next key is `x`, the
/// host should close the focused buffer tab and short-circuit the
/// rest of the dispatcher. Returns the boolean predicate so the host
/// keeps the actual `close_focused_buffer_tab` call (which is not
/// pure).
pub fn markdown_leader_x_closes_buffer_tab(
    classes: MarkdownModifierClass,
    is_x_char: bool,
) -> bool {
    classes.plain && is_x_char
}

// ---------------------------------------------------------------------------
// dispatch_markdown_key per-arm classifiers
// ---------------------------------------------------------------------------

/// Super+Z (and Shift+Super+Z) undo/redo classification. Returns
/// `Some(true)` for redo, `Some(false)` for undo, `None` if the
/// classifier does not apply (host falls through to normal dispatch).
///
/// The host still calls the underlying `MarkdownPane::undo` /
/// `MarkdownPane::redo` itself because those methods mutate the pane
/// and return whether anything changed.
pub fn markdown_super_z_intent(
    classes: MarkdownModifierClass,
    is_z_char: bool,
    shift: bool,
) -> Option<bool> {
    if !classes.super_only || !is_z_char {
        return None;
    }
    Some(shift)
}

/// Decide whether the dispatcher should issue the leader-flushed
/// scroll-by-page in normal mode. The host has already determined
/// `flushed_markdown_leader` and the current mode; this returns the
/// trailing "scroll by 86% viewport" decision so callers can swap
/// the literal for a named helper.
pub fn markdown_flushed_leader_scrolls_normal_mode(
    mode: Option<MarkdownMode>,
    flushed_markdown_leader: bool,
) -> bool {
    flushed_markdown_leader && matches!(mode, Some(MarkdownMode::Normal))
}

/// Per-arm decision for the `Ctrl + key` branch of
/// `dispatch_markdown_key`. The host still calls the matching
/// mutators on the pane (those return a `bool` that decides whether
/// the arm "handled" the key); the classifier centralises the
/// key→action mapping so the keymap can be inspected/tested
/// without invoking the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownCtrlAction {
    /// Ctrl+D — scroll cursor down by half-page.
    ScrollCursorDownHalfPage,
    /// Ctrl+U — scroll cursor up by half-page.
    ScrollCursorUpHalfPage,
    /// Ctrl+ArrowUp — move table row fast (up).
    MoveTableRowUp,
    /// Ctrl+ArrowDown — move table row fast (down).
    MoveTableRowDown,
    /// Ctrl+ArrowLeft — move table cell backward.
    MoveTableCellPrev,
    /// Ctrl+ArrowRight — move table cell forward.
    MoveTableCellNext,
    /// Ctrl+R — redo (vim / standard).
    Redo,
    /// Ctrl+V — blockwise visual (vim).
    VimBlockVisual,
    /// Ctrl+O — jumplist back (vim).
    VimJumpBack,
    /// Ctrl+I — jumplist forward (vim).
    VimJumpForward,
}

/// Resolve a Ctrl-only key event to a known [`MarkdownCtrlAction`].
///
/// Inputs are POD enums supplied by the host adapter: a `key_kind`
/// label for the modifier-stripped logical key, and the host's
/// already-classified modifier set. Returns `None` when the key does
/// not match a recognised binding (host falls through to
/// `handled = false`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkdownCtrlKeyKind {
    CharD,
    CharU,
    CharR,
    CharV,
    CharO,
    CharI,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

pub fn markdown_ctrl_action(
    classes: MarkdownModifierClass,
    key: MarkdownCtrlKeyKind,
) -> Option<MarkdownCtrlAction> {
    if !classes.ctrl_only {
        return None;
    }
    Some(match key {
        MarkdownCtrlKeyKind::CharD => MarkdownCtrlAction::ScrollCursorDownHalfPage,
        MarkdownCtrlKeyKind::CharU => MarkdownCtrlAction::ScrollCursorUpHalfPage,
        MarkdownCtrlKeyKind::ArrowUp => MarkdownCtrlAction::MoveTableRowUp,
        MarkdownCtrlKeyKind::ArrowDown => MarkdownCtrlAction::MoveTableRowDown,
        MarkdownCtrlKeyKind::ArrowLeft => MarkdownCtrlAction::MoveTableCellPrev,
        MarkdownCtrlKeyKind::ArrowRight => MarkdownCtrlAction::MoveTableCellNext,
        MarkdownCtrlKeyKind::CharR => MarkdownCtrlAction::Redo,
        MarkdownCtrlKeyKind::CharV => MarkdownCtrlAction::VimBlockVisual,
        MarkdownCtrlKeyKind::CharO => MarkdownCtrlAction::VimJumpBack,
        MarkdownCtrlKeyKind::CharI => MarkdownCtrlAction::VimJumpForward,
    })
}

/// `markdown_dispatch_finalize_decision` collects the trailing
/// branching that follows the per-mode match in
/// `dispatch_markdown_key`. The host has already mutated the pane
/// and knows whether the key was `handled` (true if any arm matched
/// and produced output), and whether the leader chord just expired.
/// This pure helper returns the small set of post-actions the
/// finalizer must take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkdownDispatchFinalize {
    /// Refresh both the block menu and the link-completion menu.
    /// Host invokes the matching mutating helpers; if either
    /// "changed" then the host can skip the trailing `mark_dirty`.
    pub refresh_menus: bool,
    /// Reset the trail-cursor animation (already required when
    /// `snap_cursor` fired earlier in the dispatch).
    pub reset_trail_cursor: bool,
    /// Sync the buffer-tabs modified flag for the active markdown
    /// document.
    pub sync_active_modified: bool,
}

/// Compute the post-match finalize step for `dispatch_markdown_key`.
///
/// * `handled` — at least one arm consumed the key.
/// * `flushed_leader` — the leader timer expired this tick.
/// * `snap_cursor` — an arm requested the trail-cursor be reset.
pub fn markdown_dispatch_finalize(
    handled: bool,
    flushed_leader: bool,
    snap_cursor: bool,
) -> Option<MarkdownDispatchFinalize> {
    if !(handled || flushed_leader) {
        return None;
    }
    Some(MarkdownDispatchFinalize {
        refresh_menus: true,
        reset_trail_cursor: snap_cursor,
        sync_active_modified: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_class_decomposes() {
        let plain = MarkdownBridgeModifiers::default();
        assert_eq!(
            plain.classify(),
            MarkdownModifierClass {
                plain: true,
                ctrl_only: false,
                super_only: false,
            }
        );
        let ctrl = MarkdownBridgeModifiers {
            control: true,
            ..Default::default()
        };
        assert!(ctrl.classify().ctrl_only);
        let super_shift = MarkdownBridgeModifiers {
            shift: true,
            super_key: true,
            ..Default::default()
        };
        assert!(super_shift.classify().super_only);
    }

    #[test]
    fn leader_expires_when_timer_exceeds_window() {
        let t = markdown_leader_transition(Some(MarkdownMode::Normal), true, 5_000);
        assert!(t.clear_pending);
        assert!(t.flushed);
    }

    #[test]
    fn leader_consumed_within_window() {
        let t = markdown_leader_transition(Some(MarkdownMode::Normal), true, 100);
        assert!(t.clear_pending);
        assert!(!t.flushed);
    }

    #[test]
    fn leader_dropped_outside_normal_mode() {
        let t = markdown_leader_transition(Some(MarkdownMode::Insert), true, 100);
        assert!(t.clear_pending);
        assert!(!t.flushed);
    }

    #[test]
    fn leader_idle_when_not_pending() {
        let t = markdown_leader_transition(Some(MarkdownMode::Normal), false, 0);
        assert!(!t.clear_pending);
        assert!(!t.flushed);
    }

    #[test]
    fn outside_workspace_refuses() {
        let path = PathBuf::from("/tmp/elsewhere/note.md");
        let root = PathBuf::from("/home/user/workspace");
        let decision = missing_markdown_note_decision(&path, Some(&root));
        assert!(matches!(decision, MissingNoteDecision::Refuse { .. }));
    }

    #[test]
    fn inside_workspace_seeds_title() {
        let path = PathBuf::from("/home/user/workspace/notes/My Topic.md");
        let root = PathBuf::from("/home/user/workspace");
        let decision = missing_markdown_note_decision(&path, Some(&root));
        match decision {
            MissingNoteDecision::Create { contents } => {
                assert!(contents.starts_with("# My Topic"));
            }
            _ => panic!("expected create"),
        }
    }

    #[test]
    fn empty_stem_falls_back_to_untitled() {
        let path = PathBuf::from("/tmp/.md");
        let decision = missing_markdown_note_decision(&path, None);
        match decision {
            MissingNoteDecision::Create { contents } => {
                assert!(contents.starts_with("# Untitled"));
            }
            _ => panic!("expected create"),
        }
    }

    #[test]
    fn dir_target_plans_file_tree_open() {
        let target = MarkdownLinkTarget {
            path: PathBuf::from("/tmp/dir"),
            line: None,
            code_ref: false,
        };
        let plan = markdown_link_open_plan(&target, true, false, true);
        assert!(matches!(
            plan,
            MarkdownLinkOpenPlan::OpenDirectoryInFileTree { .. }
        ));
    }

    #[test]
    fn missing_markdown_marked_for_creation() {
        let target = MarkdownLinkTarget {
            path: PathBuf::from("/tmp/notes/new.md"),
            line: None,
            code_ref: false,
        };
        let plan = markdown_link_open_plan(&target, false, true, false);
        match plan {
            MarkdownLinkOpenPlan::OpenMarkdownPath {
                create_missing_note,
                ..
            } => assert!(create_missing_note),
            _ => panic!("expected markdown path plan"),
        }
    }

    #[test]
    fn coderef_does_not_create_missing() {
        let target = MarkdownLinkTarget {
            path: PathBuf::from("/tmp/notes/new.md"),
            line: None,
            code_ref: true,
        };
        let plan = markdown_link_open_plan(&target, false, true, false);
        match plan {
            MarkdownLinkOpenPlan::OpenMarkdownPath {
                create_missing_note,
                ..
            } => assert!(!create_missing_note),
            _ => panic!("expected markdown path plan"),
        }
    }

    #[test]
    fn non_markdown_falls_to_editor() {
        let target = MarkdownLinkTarget {
            path: PathBuf::from("/tmp/notes/foo.txt"),
            line: Some(7),
            code_ref: false,
        };
        let plan = markdown_link_open_plan(&target, false, false, true);
        assert!(matches!(
            plan,
            MarkdownLinkOpenPlan::OpenEditorPath { line: Some(7), .. }
        ));
    }

    #[test]
    fn tab_move_workspace_stacks_existing_route() {
        let plan = markdown_tab_move_plan(Some(7), MarkdownTabDestination::Workspace);
        assert_eq!(
            plan,
            MarkdownTabMovePlan::AttachWorkspace {
                existing_route: Some(7)
            }
        );
    }

    #[test]
    fn tab_move_to_pane_without_route_ensures_pane_route() {
        let plan = markdown_tab_move_plan(None, MarkdownTabDestination::Pane(12));
        assert_eq!(
            plan,
            MarkdownTabMovePlan::EnsurePaneRoute { dest_route: 12 }
        );
    }

    #[test]
    fn editor_jump_command_uses_clamped_line() {
        let cmd = markdown_link_editor_jump_command(0, "\"/tmp/x.rs\"");
        assert!(cmd.contains("vim.api.nvim_win_set_cursor(0, { 1, 0 })"));
        assert!(cmd.contains("require('rio.search').preview(1)"));
    }

    #[test]
    fn apply_template_refresh_only_for_wiki_or_code_link() {
        assert!(apply_markdown_block_template_refreshes_link_completion(
            MarkdownBlockTemplate::WikiLink
        ));
        assert!(apply_markdown_block_template_refreshes_link_completion(
            MarkdownBlockTemplate::CodeLink
        ));
        assert!(!apply_markdown_block_template_refreshes_link_completion(
            MarkdownBlockTemplate::Heading1
        ));
        assert!(!apply_markdown_block_template_refreshes_link_completion(
            MarkdownBlockTemplate::Paragraph
        ));
    }

    #[test]
    fn link_completion_suppresses_on_line_suffix() {
        let decision = markdown_link_completion_decision(
            MarkdownWikiLinkKind::CodeRef,
            "target-12",
            false,
            None,
        );
        assert_eq!(
            decision,
            MarkdownLinkCompletionDecision::SuppressForLineSuffix
        );
    }

    #[test]
    fn link_completion_returns_no_suggestions_when_empty() {
        let decision = markdown_link_completion_decision(
            MarkdownWikiLinkKind::Note,
            "missing",
            true,
            None,
        );
        assert_eq!(decision, MarkdownLinkCompletionDecision::NoSuggestions);
    }

    #[test]
    fn link_completion_open_picks_kind_specific_title() {
        let decision = markdown_link_completion_decision(
            MarkdownWikiLinkKind::Heading,
            "intro",
            false,
            None,
        );
        match decision {
            MarkdownLinkCompletionDecision::Open { title, kind, .. } => {
                assert_eq!(title, "Link heading");
                assert_eq!(kind, MarkdownWikiLinkKind::Heading);
            }
            _ => panic!("expected open"),
        }
    }

    #[test]
    fn link_completion_open_when_only_create_target_present() {
        let decision = markdown_link_completion_decision(
            MarkdownWikiLinkKind::Note,
            "brand-new",
            true,
            Some("brand-new.md".to_string()),
        );
        match decision {
            MarkdownLinkCompletionDecision::Open {
                title,
                create_target,
                ..
            } => {
                assert_eq!(title, "Link note");
                assert_eq!(create_target.as_deref(), Some("brand-new.md"));
            }
            _ => panic!("expected open with create target"),
        }
    }

    #[test]
    fn line_suffix_only_matches_trailing_dash_digits() {
        assert!(markdown_link_line_suffix_mode("target-42"));
        assert!(!markdown_link_line_suffix_mode("-12"));
        assert!(!markdown_link_line_suffix_mode("target-12a"));
        assert!(!markdown_link_line_suffix_mode("no-suffix"));
    }

    #[test]
    fn normal_colon_opens_palette_only_when_plain_and_colon() {
        let plain = MarkdownModifierClass {
            plain: true,
            ctrl_only: false,
            super_only: false,
        };
        let ctrl = MarkdownModifierClass {
            plain: false,
            ctrl_only: true,
            super_only: false,
        };
        assert!(markdown_normal_colon_opens_palette(
            Some(MarkdownMode::Normal),
            plain,
            true
        ));
        assert!(!markdown_normal_colon_opens_palette(
            Some(MarkdownMode::Insert),
            plain,
            true
        ));
        assert!(!markdown_normal_colon_opens_palette(
            Some(MarkdownMode::Normal),
            ctrl,
            true
        ));
        assert!(!markdown_normal_colon_opens_palette(
            Some(MarkdownMode::Normal),
            plain,
            false
        ));
    }

    #[test]
    fn leader_x_only_closes_for_plain_x() {
        let plain = MarkdownModifierClass {
            plain: true,
            ctrl_only: false,
            super_only: false,
        };
        assert!(markdown_leader_x_closes_buffer_tab(plain, true));
        assert!(!markdown_leader_x_closes_buffer_tab(plain, false));
        let ctrl = MarkdownModifierClass {
            plain: false,
            ctrl_only: true,
            super_only: false,
        };
        assert!(!markdown_leader_x_closes_buffer_tab(ctrl, true));
    }

    #[test]
    fn super_z_classifies_undo_and_redo() {
        let super_only = MarkdownModifierClass {
            plain: false,
            ctrl_only: false,
            super_only: true,
        };
        assert_eq!(
            markdown_super_z_intent(super_only, true, false),
            Some(false)
        );
        assert_eq!(markdown_super_z_intent(super_only, true, true), Some(true));
        assert_eq!(markdown_super_z_intent(super_only, false, false), None);
        let plain = MarkdownModifierClass {
            plain: true,
            ctrl_only: false,
            super_only: false,
        };
        assert_eq!(markdown_super_z_intent(plain, true, false), None);
    }

    #[test]
    fn leader_flushed_scrolls_only_in_normal_mode() {
        assert!(markdown_flushed_leader_scrolls_normal_mode(
            Some(MarkdownMode::Normal),
            true
        ));
        assert!(!markdown_flushed_leader_scrolls_normal_mode(
            Some(MarkdownMode::Insert),
            true
        ));
        assert!(!markdown_flushed_leader_scrolls_normal_mode(
            Some(MarkdownMode::Normal),
            false
        ));
    }

    #[test]
    fn ctrl_action_resolves_known_bindings_and_rejects_non_ctrl() {
        let ctrl = MarkdownModifierClass {
            plain: false,
            ctrl_only: true,
            super_only: false,
        };
        assert_eq!(
            markdown_ctrl_action(ctrl, MarkdownCtrlKeyKind::CharD),
            Some(MarkdownCtrlAction::ScrollCursorDownHalfPage)
        );
        assert_eq!(
            markdown_ctrl_action(ctrl, MarkdownCtrlKeyKind::ArrowUp),
            Some(MarkdownCtrlAction::MoveTableRowUp)
        );
        assert_eq!(
            markdown_ctrl_action(ctrl, MarkdownCtrlKeyKind::CharR),
            Some(MarkdownCtrlAction::Redo)
        );
        let plain = MarkdownModifierClass {
            plain: true,
            ctrl_only: false,
            super_only: false,
        };
        assert_eq!(
            markdown_ctrl_action(plain, MarkdownCtrlKeyKind::CharD),
            None
        );
    }

    #[test]
    fn dispatch_finalize_runs_only_when_handled_or_flushed() {
        assert!(markdown_dispatch_finalize(false, false, false).is_none());
        let f = markdown_dispatch_finalize(true, false, false).unwrap();
        assert!(f.refresh_menus);
        assert!(!f.reset_trail_cursor);
        assert!(f.sync_active_modified);
        let f = markdown_dispatch_finalize(false, true, true).unwrap();
        assert!(f.reset_trail_cursor);
    }
}
