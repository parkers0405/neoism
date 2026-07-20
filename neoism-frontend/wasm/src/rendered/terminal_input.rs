use super::*;
use neoism_ui::input::TerminalShellKind;
use neoism_ui::panels::PanelContext;
use neoism_ui::services::Services;
use neoism_ui::terminal_blocks::BlockStatusKind;
use neoism_ui::PanelKey;
use std::path::PathBuf;
use web_time::Duration;

#[wasm_bindgen]
impl ChromeBridge {
    pub fn set_terminal_input(&mut self, text: &str) {
        self.replace_terminal_block_input(text);
        self.sync_terminal_input_snapshot();
    }

    pub fn clear_terminal_input(&mut self) {
        self.replace_terminal_block_input("");
        self.sync_terminal_input_snapshot();
    }

    pub fn terminal_input(&self) -> String {
        self.chrome.terminal_input().to_string()
    }

    pub fn terminal_command_composer_visible(&self) -> bool {
        self.chrome.command_composer.is_visible()
    }

    /// Whether the next printable keystroke belongs to the composer
    /// rather than the raw PTY. Mirrors the desktop
    /// `current_terminal_block_input_active` gate so typed input
    /// never splits between the composer and the shell's own line
    /// editor — in particular the fresh-terminal boot window before
    /// the first OSC 133 prompt, and while the composer already
    /// holds a pending command. Reads live shell state directly so
    /// it doesn't lag behind the render-synced visibility flag.
    pub fn terminal_should_capture_input(&self) -> bool {
        if !self.chrome.is_terminal_tab_active()
            || self.chrome.is_neoism_agent_tab_active()
        {
            return false;
        }
        let terminal = self.rendered.terminal_ref();
        let state = terminal.inner.shell_prompt_state();
        let terminal_alt_screen = terminal
            .inner
            .mode()
            .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN);
        self.terminal_blocks
            .should_capture_input(state, terminal_alt_screen)
    }

    pub fn terminal_input_insert(&mut self, text: &str) {
        self.terminal_blocks.insert_str(text);
        self.sync_terminal_input_snapshot();
    }

    pub fn terminal_input_key(&mut self, key: &str) -> bool {
        let before_text = self.terminal_blocks.text().to_string();
        let before_cursor = self.terminal_blocks.cursor_byte();
        match key {
            "Backspace" => self.terminal_blocks.backspace(),
            "Delete" => self.terminal_blocks.delete(),
            "Tab" => {
                let cwd = self.rendered.terminal_ref().inner.current_directory.clone();
                self.terminal_blocks.complete_or_accept(cwd.as_deref());
            }
            "Shift+Tab" => {
                if self.terminal_blocks.completion_menu_active() {
                    self.terminal_blocks.completion_previous();
                }
            }
            "ArrowLeft" => self.terminal_blocks.move_left(),
            "ArrowRight" => {
                if !self.terminal_blocks.accept_suggestion() {
                    self.terminal_blocks.move_right();
                }
            }
            "Home" => self.terminal_blocks.move_home(),
            "End" => {
                if !self.terminal_blocks.accept_suggestion() {
                    self.terminal_blocks.move_end();
                }
            }
            "ArrowUp" => {
                let input_text = self.terminal_blocks.text().to_string();
                let visual_ranges = self
                    .chrome
                    .command_composer
                    .input_visual_line_ranges(&input_text);
                let visual_wrapped = visual_ranges.len() > 1;
                if self.terminal_blocks.completion_menu_active() {
                    self.terminal_blocks.completion_previous();
                } else if visual_wrapped {
                    if !self
                        .terminal_blocks
                        .move_visual_up_in_ranges(&visual_ranges)
                        && !self.terminal_blocks.is_multiline()
                    {
                        self.terminal_blocks.history_previous();
                    }
                } else if !self.terminal_blocks.move_visual_up()
                    && !self.terminal_blocks.is_multiline()
                {
                    self.terminal_blocks.history_previous();
                }
            }
            "ArrowDown" => {
                let input_text = self.terminal_blocks.text().to_string();
                let visual_ranges = self
                    .chrome
                    .command_composer
                    .input_visual_line_ranges(&input_text);
                let visual_wrapped = visual_ranges.len() > 1;
                if self.terminal_blocks.completion_menu_active() {
                    self.terminal_blocks.completion_next();
                } else if visual_wrapped {
                    if !self
                        .terminal_blocks
                        .move_visual_down_in_ranges(&visual_ranges)
                        && !self.terminal_blocks.is_multiline()
                    {
                        self.terminal_blocks.history_next();
                    }
                } else if !self.terminal_blocks.move_visual_down()
                    && !self.terminal_blocks.is_multiline()
                {
                    self.terminal_blocks.history_next();
                }
            }
            _ => return false,
        }
        self.sync_terminal_input_snapshot();
        before_text != self.terminal_blocks.text()
            || before_cursor != self.terminal_blocks.cursor_byte()
            || self.terminal_blocks.completion_menu_active()
    }

    /// Seed the composer's ArrowUp history with the daemon user's
    /// shell history (oldest first). Desktop loads `~/.zsh_history`
    /// directly; web fetches it via `Files::ReadShellHistory`.
    pub fn terminal_seed_history(&mut self, entries_json: &str) {
        let Ok(entries) = serde_json::from_str::<Vec<String>>(entries_json) else {
            return;
        };
        self.terminal_blocks.set_history(entries);
    }

    /// Store a daemon-resolved directory listing for Tab
    /// completion. `entries_json` is `[[name, is_dir], …]`.
    pub fn terminal_seed_completion_dir(&mut self, dir: &str, entries_json: &str) {
        let Ok(entries) = serde_json::from_str::<Vec<(String, bool)>>(entries_json)
        else {
            return;
        };
        neoism_ui::terminal_blocks::completion::seed_host_dir_listing(
            PathBuf::from(dir),
            entries,
        );
    }

    /// Directories Tab completion wanted but had no cached listing
    /// for. JS fetches each via the daemon and seeds it back.
    pub fn drain_completion_dir_requests(&mut self) -> JsValue {
        let dirs: Vec<String> =
            neoism_ui::terminal_blocks::completion::drain_host_dir_requests()
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
        serde_wasm_bindgen::to_value(&dirs).unwrap_or(JsValue::NULL)
    }

    pub fn terminal_submit_payload(&mut self) -> Vec<u8> {
        let command = self.terminal_blocks.text().to_string();
        let output_start_row = self.terminal_output_start_row();
        let cwd = self.rendered.terminal_ref().inner.current_directory.clone();
        self.terminal_blocks
            .submit_with_context(cwd.as_deref(), output_start_row);
        self.sync_terminal_input_snapshot();
        TerminalShellKind::Zsh.command_payload(&command, false)
    }

    pub fn record_terminal_submit(&mut self, command: &str) {
        // Only record when the chrome actually owns the prompt — i.e.,
        // the command composer is visible. When it's hidden (alt-screen
        // TUI, running command, passthrough session) the Enter key is
        // destined for the foreground process, not a new shell command.
        // Recording in those states creates a spurious Running block
        // and shows the rainbow spinner inside htop / codex / claude.
        if !self.chrome.command_composer.is_visible() {
            return;
        }
        self.replace_terminal_block_input(command);
        let output_start_row = self.terminal_output_start_row();
        let cwd = self.rendered.terminal_ref().inner.current_directory.clone();
        self.terminal_blocks
            .submit_with_context(cwd.as_deref(), output_start_row);
        self.sync_terminal_input_snapshot();
    }

    pub fn terminal_command_block_count(&self) -> u32 {
        self.terminal_blocks.command_block_count() as u32
    }

    pub fn terminal_command_blocks_json(&self) -> String {
        #[derive(serde::Serialize)]
        struct DebugBlock {
            command: String,
            cwd: Option<String>,
            status: String,
            output_start_row: Option<usize>,
            duration_ms: f32,
        }

        let blocks = self
            .terminal_blocks
            .command_block_snapshots()
            .into_iter()
            .map(|block| DebugBlock {
                command: block.command,
                cwd: block.cwd,
                status: match block.status {
                    BlockStatusKind::Running => "running".to_string(),
                    BlockStatusKind::Ok => "ok".to_string(),
                    BlockStatusKind::Error(code) => format!("error:{code}"),
                },
                output_start_row: block.output_start_row,
                duration_ms: block.duration_ms,
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&blocks).unwrap_or_else(|_| "[]".to_string())
    }

    pub fn dismiss_terminal_splash(&mut self) {
        self.chrome.dismiss_terminal_splash();
        self.relayout_chrome();
    }

    pub fn reset_terminal_splash(&mut self) {
        self.chrome.reset_terminal_splash();
        self.relayout_chrome();
    }

    pub(crate) fn sync_active_tab_state(&mut self, key: usize) {
        self.active_tab_index = key;
        self.chrome.set_active_tab_index(key);
        if key != 0 {
            if let Some(tree) = self.chrome.file_tree.as_mut() {
                tree.set_focused(false);
            }
            self.chrome.blur(PanelKey::FileTree);
        }
        let content = self.tab_contents.get(&key).cloned();
        self.chrome.set_tab_content(content.clone());
        let path = self.tab_paths.get(&key).cloned();
        let lang = path
            .as_deref()
            .map(neoism_ui::syntax::Lang::from_path)
            .unwrap_or(neoism_ui::syntax::Lang::Other);
        self.chrome.set_tab_lang(lang);
        if lang == neoism_ui::syntax::Lang::Markdown {
            self.chrome.set_markdown_content(content, path.as_deref());
        } else {
            self.chrome.set_markdown_content(None, None);
        }
        self.sync_status_mode_for_active_tab(key, lang);
    }

    /// Drive the status-line mode pill + primary glyph off the now-active
    /// surface so it reads TERMINAL / MARKDOWN / AGENT / NORMAL and plays
    /// the cross-fade scramble, matching desktop's `render` mapping
    /// (agent→Agent, markdown→Markdown, else Terminal).
    /// The web never called the `set_status_mode_*` setters, so the pill
    /// was stuck on the initial `Terminal` for every tab.
    pub(crate) fn sync_status_mode_for_active_tab_index(&mut self) {
        let key = self.active_tab_index;
        let lang = self
            .tab_paths
            .get(&key)
            .map(|p| neoism_ui::syntax::Lang::from_path(p))
            .unwrap_or(neoism_ui::syntax::Lang::Other);
        self.sync_status_mode_for_active_tab(key, lang);
    }

    pub(crate) fn sync_status_mode_for_active_tab(
        &mut self,
        key: usize,
        lang: neoism_ui::syntax::Lang,
    ) {
        use neoism_ui::panels::status_line::{Mode, PrimaryKind};
        let kind = self
            .tab_kinds
            .get(&key)
            .map(String::as_str)
            .unwrap_or("terminal");
        let (mode, primary_kind) = if kind == "neoism-agent" {
            (Mode::Agent, PrimaryKind::Agent)
        } else if lang == neoism_ui::syntax::Lang::Markdown {
            (Mode::Markdown, PrimaryKind::File)
        } else if kind == "terminal" {
            (Mode::Terminal, PrimaryKind::Terminal)
        } else {
            // A non-markdown file/editor surface. NORMAL until a
            // mode push refines it (see set_status_mode_insert).
            (Mode::Normal, PrimaryKind::File)
        };
        let current = self.chrome.status_line.info();
        if current.mode != mode || current.primary_kind != primary_kind {
            let mut info = current.clone();
            info.mode = mode;
            info.primary_kind = primary_kind;
            self.chrome.status_line.set_info(info);
        }
    }

    /// Which surface should receive raw keystrokes on the next
    /// input event. `"terminal"` when the user is viewing the
    /// always-present Terminal tab; `"agent"` for the Neoism Agent
    /// tab; `"editor"` for any other buffer tab (a file surface).
    ///
    /// Exposed as a `String` rather than a `u8` discriminant so
    /// the JS host can `===` against the literal name without
    /// pulling in a wasm-bindgen enum.
    pub fn active_surface(&self) -> String {
        if self.chrome.is_neoism_agent_tab_active() {
            "agent".to_string()
        } else if self
            .tab_kinds
            .get(&self.active_tab_index)
            // Unknown kind (pre-first-replay boot) defaults to the
            // terminal surface. No index-0 special case: restored
            // strips put file tabs first and fresh terminals last.
            .map(|kind| kind == "terminal")
            .unwrap_or(true)
        {
            "terminal".to_string()
        } else {
            "editor".to_string()
        }
    }

    pub(crate) fn queue_agent_tab_open(&mut self) {
        self.pending_agent_tab_opens = self.pending_agent_tab_opens.saturating_add(1);
        self.chrome.command_palette.set_enabled(false);
        self.chrome.finder.set_enabled(false);
        self.relayout_chrome();
    }

    pub fn hide_modals(&mut self) {
        self.chrome.finder.set_enabled(false);
        self.chrome.command_palette.set_enabled(false);
        self.relayout_chrome();
    }

    /// Hit-test a click at logical-pixel coordinates against the
    /// splash overlay's menu buttons. Returns `true` when a menu
    /// action fired so the JS host can swallow the click.
    pub fn splash_click(&mut self, x: f32, y: f32) -> bool {
        let Some(idx) = self.chrome.splash_overlay.menu_hit(x, y) else {
            return false;
        };
        match idx {
            0 => {
                self.chrome.show_file_tree();
            }
            1 => {
                self.chrome.toggle_notes_sidebar();
            }
            2 => {
                self.queue_agent_tab_open();
            }
            3 => self.chrome.finder.set_enabled(true),
            4 => self.chrome.command_palette.set_enabled(true),
            _ => return false,
        }
        self.relayout_chrome();
        true
    }

    /// Update the splash overlay's hover cursor for paint-time
    /// menu highlight + wordmark fidget tracking.
    pub fn splash_mouse_move(&mut self, x: f32, y: f32) {
        self.chrome.splash_overlay.set_mouse(Some((x, y)));
    }

    pub fn splash_mouse_leave(&mut self) {
        self.chrome.splash_overlay.set_mouse(None);
    }

    /// Pop the wordmark fidget (squash + ripple) at a click point.
    pub fn splash_wordmark_click(&mut self, x: f32, y: f32) {
        if self.chrome.splash_overlay.wordmark_hit(x, y) {
            self.chrome.splash_overlay.pop_click(x, y);
        }
    }

    /// Toggle the file-tree sidebar using the desktop semantics:
    /// hidden -> show+focus, visible+focused -> hide,
    /// visible+unfocused -> focus.
    pub fn toggle_file_tree(&mut self) {
        self.chrome.toggle_file_tree();
        self.relayout_chrome();
    }

    /// Force the file-tree sidebar open (idempotent).
    pub fn show_file_tree(&mut self) {
        self.chrome.show_file_tree();
        self.relayout_chrome();
    }

    /// Force the file-tree sidebar closed.
    pub fn hide_file_tree(&mut self) {
        self.chrome.hide_file_tree();
        self.relayout_chrome();
    }

    pub fn show_command_composer(&mut self) {
        self.chrome.command_composer.set_visible(true);
        self.relayout_chrome();
    }

    pub fn show_git_diff(&mut self) {
        self.chrome.git_diff.show();
        let theme = self.chrome.theme().clone();
        let services = Services {
            files: &*self.files,
            clipboard: &*self.clipboard,
            commands: &*self.commands,
            git: &*self.git,
            clock: &*self.clock,
            search: &*self.search,
            notifications: &*self.notifications,
        };
        let mut ctx = PanelContext {
            services,
            theme: &theme,
            time: Duration::from_micros(
                (self.services_state.0.borrow().now_ms * 1000.0).max(0.0) as u64,
            ),
        };
        self.chrome.git_diff.refresh(&mut ctx);
        self.relayout_chrome();
    }

    pub fn toggle_git_diff(&mut self) {
        if self.chrome.git_diff.is_visible() {
            self.chrome.git_diff.hide();
            self.relayout_chrome();
        } else {
            self.show_git_diff();
        }
    }
}
