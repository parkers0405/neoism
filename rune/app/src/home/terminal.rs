//! One Home terminal: its shell and the view state around it, with the presentation rules of
//! `gpuix/src/terminal/BlockTerminalView.tsx` (when the pane shows blocks or the raw grid, when
//! the composer steps aside for a running command).

use super::blocks::Shell;
use super::composer::Composer;
use crate::ghostty::{Colors, Screen};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A command that has not finished after this long hides the composer and takes the keyboard.
pub const LONG_RUNNING: Duration = Duration::from_millis(50);

pub struct Terminal {
    pub shell: Shell,
    pub composer: Composer,
    /// The measured pane (blocks area plus composer), in points.
    pub area: [f32; 2],
    /// The blocks viewport's `(offset, max)`, mirrored from `on_scroll_offset`.
    pub scroll: Rc<Cell<(f32, f32)>>,
    /// Where notched wheel input is gliding the blocks to.
    pub wheel_target: Option<f32>,
    /// When the current submission or command started (for `LONG_RUNNING`).
    pub busy_since: Option<Instant>,
    /// The block under the pointer, and the one clicked (tinted, as GPUiX selects).
    pub hovered: Option<u64>,
    pub selected: Option<u64>,
    /// The grid as of the last sync, while the pane shows it.
    pub screen: Option<Screen>,
    /// A grid-presentation shell (`terminal.block-ui: false`): always Ghostty's grid.
    pub grid: bool,
    /// The block whose actions menu is open (`moreId`).
    pub more: Option<u64>,
    /// The block whose output is being filtered, and the filter query (`filterId`, `filter`).
    pub filter: Option<(u64, rune::TextEdit)>,
    /// Blocks whose output is collapsed.
    pub collapsed: std::collections::HashSet<u64>,
    /// A coding-agent TUI running in the foreground (`claude`, `codex`, `opencode`): the tab
    /// shows its logo and name until it exits (`SessionProgram`).
    pub program: Option<&'static str>,
}

/// The agent TUIs whose tabs take their name and logo (`model.ts`, `SessionProgram`).
pub fn agent_program(program: &str) -> Option<&'static str> {
    let first = program.split_whitespace().next().unwrap_or("");
    let name = first.rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
    match name.as_str() {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        _ => None,
    }
}

/// The tab title for an agent TUI.
pub fn program_title(program: &str) -> &'static str {
    match program { "claude" => "Claude", "codex" => "Codex", "opencode" => "OpenCode", _ => "Terminal" }
}

impl Terminal {
    /// The block that shows the actions toolbar: an open menu, else the hovered, selected or
    /// filtered block (`toolbar`).
    pub fn toolbar_block(&self) -> Option<u64> {
        self.more.or(self.hovered).or(self.selected).or(self.filter.as_ref().map(|(id, _)| *id))
    }

    /// A block's visible output rows: none when collapsed, the matching ones while filtered.
    pub fn visible_lines<'a>(&self, block: &'a super::blocks::Block) -> Vec<&'a crate::ghostty::Row> {
        if self.collapsed.contains(&block.id) { return Vec::new(); }
        match &self.filter {
            Some((id, query)) if *id == block.id && !query.text.is_empty() => {
                let needle = query.text.to_lowercase();
                block.lines.iter().filter(|row| crate::ghostty::row_text(row).to_lowercase().contains(&needle)).collect()
            }
            _ => block.lines.iter().collect(),
        }
    }

    pub fn new(cwd: String, colors: Colors) -> Terminal {
        Terminal {
            shell: Shell::new(cwd, colors),
            composer: Composer::default(),
            area: [0.0; 2],
            scroll: Rc::default(),
            wheel_target: None,
            busy_since: None,
            hovered: None,
            selected: None,
            screen: None,
            grid: false,
            more: None,
            filter: None,
            collapsed: Default::default(),
            program: None,
        }
    }

    /// Per frame: re-read the live block (and the grid while it shows). Returns whether anything
    /// visible changed.
    pub fn sync(&mut self) -> bool {
        let output = self.shell.sync();
        let was = self.program;
        let busy = self.tick();
        // The submitted command names the program before (or without) the daemon's report.
        if self.program.is_none() && self.shell.busy() {
            self.program = self.shell.live.as_ref().and_then(|block| agent_program(&block.command));
        }
        let busy = busy || was != self.program;
        let raw = self.raw();
        if raw && (output || self.screen.is_none()) {
            self.screen = self.shell.screen();
        } else if !raw {
            self.screen = None;
        }
        output || busy
    }

    /// A theme change: Ghostty re-colours what it draws next.
    pub fn set_colors(&mut self, colors: &Colors) {
        self.shell.set_colors(colors);
        self.screen = None;
    }

    pub fn long_running(&self) -> bool {
        self.busy_since.is_some_and(|since| since.elapsed() >= LONG_RUNNING)
    }

    /// The pane shows Ghostty's grid instead of blocks: a full-screen program, a REPL in
    /// application-cursor mode, a submission the shell has not picked up, or a shell that is not
    /// at a marked prompt at all.
    pub fn raw(&self) -> bool {
        let shell = &self.shell;
        self.grid
            || shell.error.is_some()
            || shell.alt()
            || shell.running() && shell.app_cursor()
            || shell.pending() && self.long_running()
            || self.started() && !shell.ready && !shell.busy()
    }

    /// The shell has printed a marked prompt at least once (before that, Home shows its splash).
    fn started(&self) -> bool {
        !self.shell.blocks.is_empty() || self.shell.live.is_some() || self.shell.ready || self.shell.busy()
    }

    /// Keystrokes go straight to the PTY (the composer is hidden or a grid is showing).
    pub fn direct(&self) -> bool {
        self.raw() || self.shell.busy() && self.long_running()
    }

    /// Track busy time after output or a submission; returns whether presentation may change soon.
    pub fn tick(&mut self) -> bool {
        match (self.shell.busy(), self.busy_since) {
            (true, None) => { self.busy_since = Some(Instant::now()); true }
            // Back at the prompt: whatever program ran has exited.
            (false, Some(_)) => { self.busy_since = None; self.program = None; true }
            (busy, _) => busy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_tuis_are_recognised_by_their_program() {
        assert_eq!(agent_program("/usr/local/bin/codex"), Some("codex"));
        assert_eq!(agent_program("claude --resume"), Some("claude"));
        assert_eq!(agent_program("opencode"), Some("opencode"));
        assert_eq!(agent_program("zsh"), None);
        assert_eq!(program_title("claude"), "Claude");
    }
}
