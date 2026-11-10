//! Block actions (`BlockTerminalView.tsx`, `terminal-block-actions`): filter a block's output,
//! copy its command and output, put its command back in the composer, collapse its output.

use super::view::BlockAction;
use crate::app::App;
use crate::ghostty::row_text;
use rune::Tx;

impl App {
    /// Headless scenes: finished blocks in the active terminal, which gets a stand-in shell so
    /// it can own the working directory.
    pub fn fixture_blocks(&mut self, blocks: &[(&str, &[&str], i32)]) {
        let tab_id = self.active_tab().id;
        let cwd = self.cwd.clone();
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        term.shell.session = Some(format!("fixture-{tab_id}"));
        // At a marked prompt, so the pane shows blocks rather than the raw grid.
        term.shell.ready = true;
        for (index, (command, lines, exit)) in blocks.iter().enumerate() {
            let rows = lines.iter().map(|line| vec![crate::ghostty::Run {
                text: (*line).to_owned(), col: 0, width: line.chars().count(), fg: None, bg: None, bold: false, italic: false, underline: false,
            }]).collect();
            term.shell.blocks.push(super::blocks::Block {
                id: 9000 + index as u64, command: (*command).to_owned(), cwd: cwd.clone(), started: std::time::Instant::now(),
                elapsed_ms: Some(40 + index as u128 * 310), exit: Some(*exit), lines: rows, background: false, cursor: None, truncated: false,
            });
        }
    }

    /// Filter output: open the block's filter (focused, empty), or close it if it is open.
    pub fn toggle_block_filter(&mut self, tab_id: u64, block_id: u64) {
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        term.more = None;
        if term.filter.as_ref().is_some_and(|(id, _)| *id == block_id) {
            self.close_block_filter(tab_id);
            return;
        }
        let mut query = rune::TextEdit::new("");
        query.focused = true;
        term.filter = Some((block_id, query));
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) { tab.draft.focused = false; }
    }

    /// Esc in the filter, or the toolbar toggle: drop the query and give the composer its keys.
    pub fn close_block_filter(&mut self, tab_id: u64) {
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        term.filter = None;
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) { tab.draft.focused = true; }
    }

    /// Whether a block filter in the active terminal owns the keyboard.
    pub fn block_filter_focused(&self) -> bool {
        self.terminals.get(&self.active_tab().id).is_some_and(|t| t.filter.as_ref().is_some_and(|(_, q)| q.focused))
    }

    pub fn block_action(&mut self, tab_id: u64, block_id: u64, action: BlockAction, tx: &mut Tx) {
        let Some(term) = self.terminals.get_mut(&tab_id) else { return };
        term.more = None;
        let Some(block) = term.shell.blocks.iter().chain(term.shell.live.iter()).find(|b| b.id == block_id) else { return };
        let output = |rows: &[crate::ghostty::Row]| rows.iter().map(|r| row_text(r).trim_end().to_owned()).collect::<Vec<_>>().join("\n");
        match action {
            BlockAction::CopyCommand => tx.copy(block.command.clone()),
            // Copy output takes what the block shows (filtered rows while filtering).
            BlockAction::CopyOutput => {
                let shown: Vec<crate::ghostty::Row> = term.visible_lines(block).into_iter().cloned().collect();
                tx.copy(output(&shown));
            }
            BlockAction::CopyAll => {
                let text = [block.command.clone(), output(&block.lines)].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n");
                tx.copy(text);
            }
            BlockAction::Reuse => {
                let command = block.command.clone();
                if command.is_empty() { return; }
                if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
                    tab.draft.replace_text(&command);
                    tab.draft.focused = true;
                }
            }
            BlockAction::Collapse => {
                if !term.collapsed.remove(&block_id) { term.collapsed.insert(block_id); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::tabs::TabKind;

    fn shell(app: &mut App) -> u64 {
        let tab = app.active_tab().id;
        if let Some(term) = app.terminals.get_mut(&tab) { term.shell.session = Some(format!("s{tab}")); }
        tab
    }

    #[test]
    fn the_first_shell_owns_the_directory_until_the_star_moves_it() {
        let mut app = App::new();
        let home = shell(&mut app);
        app.open_tab(TabKind::Terminal);
        let second = shell(&mut app);
        assert_eq!(app.main_terminal_tab(), Some(home));
        app.set_main_terminal(second);
        assert_eq!(app.main_terminal_tab(), Some(second));
        // A closed owner hands the directory back to the first shell.
        app.terminals.remove(&second);
        assert_eq!(app.main_terminal_tab(), Some(home));
    }

    #[test]
    fn only_the_owning_terminal_moves_the_workspace_view() {
        let mut app = App::new();
        let home = shell(&mut app);
        app.open_tab(TabKind::Terminal);
        let second = shell(&mut app);
        app.terminals.get_mut(&second).unwrap().shell.cwd = "/tmp".into();
        assert!(!app.follow_main_terminal(), "a secondary terminal's cd leaves the workspace alone");
        app.select_tab(home);
        app.terminals.get_mut(&home).unwrap().shell.cwd = "/usr".into();
        assert!(app.follow_main_terminal());
        assert_eq!(app.cwd, "/usr");
        assert_eq!(app.workspace_name, "usr");
    }

    #[test]
    fn filters_and_collapse_shape_a_blocks_output() {
        let mut app = App::new();
        let tab = app.active_tab().id;
        app.fixture_blocks(&[("ls", &["a.rs", "b.txt", "c.rs"], 0)]);
        app.toggle_block_filter(tab, 9000);
        let term = app.terminals.get_mut(&tab).unwrap();
        term.filter.as_mut().unwrap().1.replace_text("RS");
        let block = term.shell.blocks[0].clone();
        assert_eq!(term.visible_lines(&block).len(), 2, "case-insensitive substring match");
        term.collapsed.insert(9000);
        assert!(term.visible_lines(&block).is_empty());
        app.close_block_filter(tab);
        assert!(app.terminals[&tab].filter.is_none());
    }

    #[test]
    fn home_and_root_are_the_dashboard() {
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(crate::agent::catalog_scope(&home), None);
        assert_eq!(crate::agent::catalog_scope("/"), None);
        assert_eq!(crate::agent::catalog_scope("/work/app/"), Some("/work/app".into()));
    }
}
