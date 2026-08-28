use super::*;

/// One recorded editor location: the file plus the cursor it was left at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavLocation {
    pub path: std::path::PathBuf,
    pub line: usize,
    pub col: usize,
}

/// Per-window back/forward navigation history over editor locations —
/// the Zed model. Jump-class actions (opening a file from the tree,
/// palette, or agent; goto-definition; tab switches) push the departed
/// location onto `back` and clear `forward`; the chrome arrows and
/// mouse back/forward buttons walk the stacks.
#[derive(Default)]
pub struct NavHistory {
    back: Vec<NavLocation>,
    forward: Vec<NavLocation>,
    /// Set while a back/forward navigation is replaying an entry so the
    /// open-file hooks don't record the replay as a fresh jump.
    suppress: bool,
}

const NAV_HISTORY_CAP: usize = 100;
/// Same-file cursor jumps at least this many lines count as a jump
/// worth returning to (mirrors Zed's teleport threshold).
const SAME_FILE_JUMP_LINES: usize = 10;

impl NavHistory {
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

impl Screen<'_> {
    /// The active editor location, if the focused workspace surface is a
    /// file-backed editor. Terminal / agent / helper tabs have no
    /// location and never enter the history.
    pub(crate) fn current_nav_location(&self) -> Option<NavLocation> {
        let context = self.context_manager.current();
        if let Some(code) = context.code.as_ref() {
            return Some(NavLocation {
                path: code.path.clone(),
                line: code.buffer.cursor_line,
                col: code.buffer.cursor_col,
            });
        }
        if let Some(markdown) = context.markdown.as_ref() {
            return Some(NavLocation {
                path: markdown.path.clone(),
                line: 0,
                col: 0,
            });
        }
        None
    }

    /// Record the location being departed because `target` is about to
    /// open. Same-file re-opens don't record unless the jump moves the
    /// cursor far enough to be worth returning to (`target_line` from
    /// goto-definition-style jumps).
    pub(crate) fn record_nav_departure(
        &mut self,
        target: &std::path::Path,
        target_line: Option<usize>,
    ) {
        if self.nav_history.suppress {
            return;
        }
        let Some(current) = self.current_nav_location() else {
            return;
        };
        if current.path == target {
            let far_enough = target_line.is_some_and(|line| {
                line.abs_diff(current.line) >= SAME_FILE_JUMP_LINES
            });
            if !far_enough {
                return;
            }
        }
        if self.nav_history.back.last() == Some(&current) {
            return;
        }
        self.nav_history.back.push(current);
        if self.nav_history.back.len() > NAV_HISTORY_CAP {
            self.nav_history.back.remove(0);
        }
        self.nav_history.forward.clear();
        self.sync_nav_arrows();
    }

    pub(crate) fn navigate_nav_back(&mut self) -> bool {
        let Some(target) = self.nav_history.back.pop() else {
            return false;
        };
        if let Some(current) = self.current_nav_location() {
            if current != target {
                self.nav_history.forward.push(current);
            }
        }
        self.replay_nav_location(&target);
        true
    }

    pub(crate) fn navigate_nav_forward(&mut self) -> bool {
        let Some(target) = self.nav_history.forward.pop() else {
            return false;
        };
        if let Some(current) = self.current_nav_location() {
            if current != target {
                self.nav_history.back.push(current);
            }
        }
        self.replay_nav_location(&target);
        true
    }

    fn replay_nav_location(&mut self, target: &NavLocation) {
        self.nav_history.suppress = true;
        if crate::editor::markdown::state::is_markdown_path(&target.path) {
            self.open_path_in_markdown(target.path.clone());
        } else {
            self.open_code_location(target.path.clone(), target.line, target.col);
        }
        self.nav_history.suppress = false;
        self.sync_nav_arrows();
        self.mark_dirty();
    }

    /// Push the stacks' emptiness into the renderer so the chrome
    /// arrows dim correctly. Called whenever the history changes.
    pub(crate) fn sync_nav_arrows(&mut self) {
        self.renderer.nav_back_enabled = self.nav_history.can_go_back();
        self.renderer.nav_forward_enabled = self.nav_history.can_go_forward();
    }
}
