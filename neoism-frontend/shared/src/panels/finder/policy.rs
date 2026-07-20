use std::path::PathBuf;

use neoism_terminal_core::crosswords::pos::Pos;
use neoism_terminal_core::crosswords::search::Match;

use super::FinderMode;
use crate::editor::markdown::is_markdown_path;

/// Inputs needed to choose how the desktop bridge should react to a
/// finder result selection: path, optional line target, and the mode
/// the finder was in (so we know whether the query needs to seed
/// `hlsearch` etc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinderOpenRequest {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub mode: FinderMode,
    pub query: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchCloseIntent {
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatchSelection {
    pub start: Pos,
    pub end: Pos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchCloseAction {
    Exit,
    ResetViState,
    SelectFocusedMatch(SearchMatchSelection),
}

pub fn focused_match_selection(focused_match: &Match) -> SearchMatchSelection {
    SearchMatchSelection {
        start: *focused_match.start(),
        end: *focused_match.end(),
    }
}

pub fn search_close_action(
    intent: SearchCloseIntent,
    vi_mode: bool,
    focused_match: Option<&Match>,
) -> SearchCloseAction {
    match (intent, vi_mode, focused_match) {
        (SearchCloseIntent::Confirm, true, _) => SearchCloseAction::Exit,
        (SearchCloseIntent::Cancel, true, _) => SearchCloseAction::ResetViState,
        (_, false, Some(focused_match)) => {
            SearchCloseAction::SelectFocusedMatch(focused_match_selection(focused_match))
        }
        (_, false, None) => SearchCloseAction::Exit,
    }
}

/// Pure inputs for the finder's cwd-resolution fallback chain. Each
/// optional field is the candidate from the corresponding stage; the
/// first one set wins, otherwise we fall back to `working_dir_config`
/// (config-level default) and finally the caller-supplied `fallback`
/// (typically `std::env::current_dir()` on the desktop / `"/"` on web).
#[derive(Debug, Clone)]
pub struct FinderCwdInputs {
    pub active_pane_workspace_root: Option<PathBuf>,
    pub active_workspace_root: Option<PathBuf>,
    pub working_dir_config: Option<PathBuf>,
    pub fallback: PathBuf,
}

pub fn finder_cwd_decision(inputs: FinderCwdInputs) -> PathBuf {
    inputs
        .active_pane_workspace_root
        .or(inputs.active_workspace_root)
        .or(inputs.working_dir_config)
        .unwrap_or(inputs.fallback)
}

/// What kind of editor we should dispatch a freshly-selected finder
/// result into. The desktop side is responsible for actually steering
/// the buffer (markdown viewer vs code pane) — this enum just captures the
/// branching decision so it can be unit-tested without touching the
/// renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinderOpenAction {
    /// Result is a markdown file — open in the markdown viewer and
    /// optionally jump to `line`.
    OpenMarkdown { path: PathBuf, line: Option<u32> },
    /// Result is a buffer + line, with optional grep query to seed
    /// `hlsearch`. Caller decides which route receives the command.
    EditAtLine {
        path: PathBuf,
        line: u32,
        target_route: Option<usize>,
        grep_query: Option<String>,
        is_git: bool,
    },
    /// Result is a plain file (no line target).
    EditFile {
        path: PathBuf,
        target_route: Option<usize>,
    },
}

/// Compute the open-action for a finder selection. `target_route` is the
/// caller's chosen editor route (the cached `finder_target_route`,
/// `None` when no editor route is targeted).
pub fn plan_finder_open(
    request: FinderOpenRequest,
    target_route: Option<usize>,
) -> FinderOpenAction {
    let FinderOpenRequest {
        path,
        line,
        mode,
        query,
    } = request;

    if is_markdown_path(&path) {
        return FinderOpenAction::OpenMarkdown { path, line };
    }

    match line {
        Some(line) => {
            let trimmed = query.trim();
            let grep_query = matches!(mode, FinderMode::Grep)
                .then_some(trimmed)
                .filter(|q| !q.is_empty())
                .map(|q| q.to_owned());
            let is_git = matches!(mode, FinderMode::GitChanges);
            FinderOpenAction::EditAtLine {
                path,
                line,
                target_route,
                grep_query,
                is_git,
            }
        }
        None => FinderOpenAction::EditFile { path, target_route },
    }
}

/// What `search_input` should do for a typed character given the
/// current search-history cursor position. Lifted here so the desktop
/// keystroke handler stays a thin dispatcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchInputAction {
    /// User isn't actively editing the search regex (no history index).
    Ignore,
    /// `c` is non-printable; drop the keystroke entirely.
    IgnoreNonPrintable,
    /// User is browsing history (index > 0). Copy the historic entry
    /// down to slot 0, then apply `edit`.
    PromoteHistory {
        source_index: usize,
        edit: SearchEdit,
    },
    /// User is already editing slot 0; apply `edit` in place.
    Apply { edit: SearchEdit },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEdit {
    /// `\x08` / `\x7f` — pop the last char.
    Backspace,
    /// Printable char (ascii ' '..='~' or unicode '\u{a0}'..).
    Push(char),
}

pub fn search_input_action(c: char, history_index: Option<usize>) -> SearchInputAction {
    let Some(index) = history_index else {
        return SearchInputAction::Ignore;
    };
    let edit = match c {
        '\x08' | '\x7f' => SearchEdit::Backspace,
        ' '..='~' | '\u{a0}'..='\u{10ffff}' => SearchEdit::Push(c),
        _ => return SearchInputAction::IgnoreNonPrintable,
    };
    if index == 0 {
        SearchInputAction::Apply { edit }
    } else {
        SearchInputAction::PromoteHistory {
            source_index: index,
            edit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_terminal_core::crosswords::pos::{Column, Line};

    fn pos(line: i32, col: usize) -> Pos {
        Pos::new(Line(line), Column(col))
    }

    #[test]
    fn cwd_uses_active_pane_workspace_root_first() {
        let cwd = finder_cwd_decision(FinderCwdInputs {
            active_pane_workspace_root: Some(PathBuf::from("/a")),
            active_workspace_root: Some(PathBuf::from("/b")),
            working_dir_config: Some(PathBuf::from("/e")),
            fallback: PathBuf::from("/f"),
        });
        assert_eq!(cwd, PathBuf::from("/a"));
    }

    #[test]
    fn cwd_falls_through_to_config_when_no_workspace() {
        let cwd = finder_cwd_decision(FinderCwdInputs {
            active_pane_workspace_root: None,
            active_workspace_root: None,
            working_dir_config: Some(PathBuf::from("/e")),
            fallback: PathBuf::from("/f"),
        });
        assert_eq!(cwd, PathBuf::from("/e"));
    }

    #[test]
    fn cwd_uses_fallback_when_everything_empty() {
        let cwd = finder_cwd_decision(FinderCwdInputs {
            active_pane_workspace_root: None,
            active_workspace_root: None,
            working_dir_config: None,
            fallback: PathBuf::from("/tmp"),
        });
        assert_eq!(cwd, PathBuf::from("/tmp"));
    }

    #[test]
    fn open_action_routes_markdown_to_viewer() {
        let action = plan_finder_open(
            FinderOpenRequest {
                path: PathBuf::from("/repo/notes.md"),
                line: Some(42),
                mode: FinderMode::Files,
                query: String::new(),
            },
            Some(3),
        );
        assert_eq!(
            action,
            FinderOpenAction::OpenMarkdown {
                path: PathBuf::from("/repo/notes.md"),
                line: Some(42),
            }
        );
    }

    #[test]
    fn open_action_emits_edit_at_line_with_grep_highlight() {
        let action = plan_finder_open(
            FinderOpenRequest {
                path: PathBuf::from("/repo/src/lib.rs"),
                line: Some(17),
                mode: FinderMode::Grep,
                query: "  vec[0]  ".to_owned(),
            },
            Some(2),
        );
        assert_eq!(
            action,
            FinderOpenAction::EditAtLine {
                path: PathBuf::from("/repo/src/lib.rs"),
                line: 17,
                target_route: Some(2),
                grep_query: Some("vec[0]".to_owned()),
                is_git: false,
            }
        );
    }

    #[test]
    fn open_action_skips_grep_highlight_when_query_blank() {
        let action = plan_finder_open(
            FinderOpenRequest {
                path: PathBuf::from("/repo/src/lib.rs"),
                line: Some(1),
                mode: FinderMode::Grep,
                query: "   ".to_owned(),
            },
            None,
        );
        let FinderOpenAction::EditAtLine { grep_query, .. } = action else {
            panic!("expected EditAtLine");
        };
        assert!(grep_query.is_none());
    }

    #[test]
    fn open_action_marks_git_changes() {
        let action = plan_finder_open(
            FinderOpenRequest {
                path: PathBuf::from("/repo/src/lib.rs"),
                line: Some(5),
                mode: FinderMode::GitChanges,
                query: String::new(),
            },
            Some(1),
        );
        let FinderOpenAction::EditAtLine { is_git, .. } = action else {
            panic!("expected EditAtLine");
        };
        assert!(is_git);
    }

    #[test]
    fn open_action_no_line_yields_edit_file() {
        let action = plan_finder_open(
            FinderOpenRequest {
                path: PathBuf::from("/repo/Cargo.toml"),
                line: None,
                mode: FinderMode::Files,
                query: String::new(),
            },
            None,
        );
        assert_eq!(
            action,
            FinderOpenAction::EditFile {
                path: PathBuf::from("/repo/Cargo.toml"),
                target_route: None,
            }
        );
    }

    #[test]
    fn search_input_ignores_when_history_inactive() {
        assert_eq!(search_input_action('a', None), SearchInputAction::Ignore);
    }

    #[test]
    fn search_input_drops_non_printable() {
        assert_eq!(
            search_input_action('\x01', Some(0)),
            SearchInputAction::IgnoreNonPrintable
        );
    }

    #[test]
    fn search_input_applies_in_place_at_slot_zero() {
        assert_eq!(
            search_input_action('q', Some(0)),
            SearchInputAction::Apply {
                edit: SearchEdit::Push('q'),
            }
        );
        assert_eq!(
            search_input_action('\x7f', Some(0)),
            SearchInputAction::Apply {
                edit: SearchEdit::Backspace,
            }
        );
    }

    #[test]
    fn search_input_promotes_history_when_browsing() {
        assert_eq!(
            search_input_action('x', Some(2)),
            SearchInputAction::PromoteHistory {
                source_index: 2,
                edit: SearchEdit::Push('x'),
            }
        );
    }

    #[test]
    fn close_search_keeps_confirm_in_vi_mode_as_exit_only() {
        let focused_match = pos(2, 3)..=pos(2, 8);

        assert_eq!(
            search_close_action(SearchCloseIntent::Confirm, true, Some(&focused_match)),
            SearchCloseAction::Exit
        );
    }

    #[test]
    fn close_search_resets_vi_state_on_cancel() {
        let focused_match = pos(2, 3)..=pos(2, 8);

        assert_eq!(
            search_close_action(SearchCloseIntent::Cancel, true, Some(&focused_match)),
            SearchCloseAction::ResetViState
        );
    }

    #[test]
    fn close_search_selects_focused_match_outside_vi_mode() {
        let focused_match = pos(2, 3)..=pos(2, 8);
        let expected = SearchCloseAction::SelectFocusedMatch(SearchMatchSelection {
            start: pos(2, 3),
            end: pos(2, 8),
        });

        assert_eq!(
            search_close_action(SearchCloseIntent::Confirm, false, Some(&focused_match)),
            expected
        );
        assert_eq!(
            search_close_action(SearchCloseIntent::Cancel, false, Some(&focused_match)),
            expected
        );
    }

    #[test]
    fn close_search_without_match_exits_outside_vi_mode() {
        assert_eq!(
            search_close_action(SearchCloseIntent::Confirm, false, None),
            SearchCloseAction::Exit
        );
        assert_eq!(
            search_close_action(SearchCloseIntent::Cancel, false, None),
            SearchCloseAction::Exit
        );
    }
}
