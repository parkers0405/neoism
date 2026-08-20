use super::*;
use neoism_terminal_core::crosswords::grid::Scroll;
use neoism_terminal_core::crosswords::pos::{Column, Line, Pos, Side};
use neoism_terminal_core::crosswords::Mode;
use neoism_terminal_core::selection::SelectionType;
use neoism_ui::editor::scroll_model::{
    TerminalAlternateScrollCsi, TerminalMouseModeWheelReport,
};
use neoism_ui::editor::selection_model::{
    apply_selection_update, hyperlink_span_at, left_click_selection_action,
    selected_text, selection_with_range, should_open_file_link_on_click,
    terminal_file_link_probe, terminal_wrapped_link_probe, LeftClickSelectionAction,
    SelectionClickKind, SelectionEndpoint, SelectionModifiers,
};
use neoism_ui::hint_policy::{
    detect_web_link_in_wrapped_row, hint_text_is_url, link_path_existence,
    split_file_link_line_suffix, terminal_file_link_token_at, web_hint_line_spans,
    LinkPathExistence,
};
use neoism_ui::hint_state::{HintState, SimpleHintConfig};
use neoism_ui::input::TerminalShellKind;
use neoism_ui::mouse_policy::{
    cell_side_by_pos, encode_normal_mouse_report, encode_sgr_mouse_report,
    mouse_report_legacy_button_byte, mouse_report_modifier_bits,
};
use neoism_ui::panels::PanelContext;
use neoism_ui::services::Services;
use neoism_ui::terminal_blocks::BlockStatusKind;
use neoism_ui::PanelKey;
use std::path::{Path, PathBuf};
use web_time::Duration;

/// Bit flags returned by the terminal wheel/pointer entry points.
/// 0 = not handled: the JS host routes the event to chrome instead.
const TERM_INPUT_HANDLED: u32 = 1;
/// Mouse-report / CSI bytes queued — drain `take_terminal_pointer_bytes`
/// into the PTY.
const TERM_INPUT_HAS_PTY: u32 = 2;
/// A selection drag started — start the 15ms autoscroll tick and
/// capture the pointer.
const TERM_INPUT_SELECTING: u32 = 4;
/// A link click / hint fire queued open intents — drain
/// `take_terminal_link_opens` (`terminal_drain_link_opens`).
const TERM_INPUT_LINK_OPEN: u32 = 8;

/// Single web terminal pane key into the shared `TerminalScroll` map.
const TERMINAL_SCROLL_PANE: usize = 0;
/// Desktop double/triple click chain threshold
/// (app/window_event/mouse.rs:61).
const CLICK_CHAIN_THRESHOLD_MS: f64 = 300.0;
/// `config.terminal.scroll` defaults (desktop `input::mouse::Mouse`).
const SCROLL_MULTIPLIER: f64 = 3.0;
const SCROLL_DIVIDER: f64 = 1.0;

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
            .should_capture_input(state, terminal_alt_screen, false)
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
            // Desktop composer key surface (desktop
            // screen/lifecycle/block_overlay.rs:713-935). Every arm
            // below is CONSUMED by the composer while it owns the
            // line — none of these bytes may reach the raw PTY, or
            // the shell's readline mutates an invisible buffer.
            "Shift+Enter" => self.terminal_blocks.insert_str("\n"),
            "Ctrl+A" => self.terminal_blocks.move_home(),
            "Ctrl+E" => self.terminal_blocks.move_end(),
            "Ctrl+K" => self.terminal_blocks.delete_to_end(),
            "Ctrl+U" => self.terminal_blocks.clear(),
            "Ctrl+W" => self.terminal_blocks.delete_previous_word(),
            // Ctrl+D with a NON-empty composer deletes forward like
            // desktop; the empty-composer EOF case is decided by the
            // JS router (the 0x04 goes straight to the PTY there).
            "Ctrl+D" => self.terminal_blocks.delete(),
            "Ctrl+C" => {
                // While the composer owns the prompt there is no
                // foreground readline to interrupt: show the ^C
                // notice (clearing pending text first) and swallow
                // the ETX (desktop block_overlay.rs:756-777).
                if !self.terminal_blocks.is_empty() {
                    self.terminal_blocks.clear();
                }
                self.terminal_blocks.show_interrupt_notice();
            }
            "Ctrl+L" => {
                // The JS router writes the 0x0c form-feed to the PTY;
                // here we drop the block-card history and re-anchor
                // the viewport to the live tail, matching desktop's
                // clear_all_blocks + clear_block_cursor + reset_wheel
                // (block_overlay.rs:810-824). Composer text survives,
                // as on desktop.
                self.terminal_blocks.clear_all_blocks();
                self.rendered.terminal_mut().inner.scroll_display(
                    neoism_terminal_core::crosswords::grid::Scroll::Bottom,
                );
                self.rendered
                    .terminal_scroll
                    .reset_wheel(TERMINAL_SCROLL_PANE);
            }
            "Ctrl+R" => {
                // Desktop consumes Ctrl+R whether or not the picker
                // had history to show (block_overlay.rs:778-788).
                let _ = self.terminal_blocks.open_history_picker();
            }
            "Ctrl+F" => {
                if !self.terminal_blocks.open_favorite_picker() {
                    self.chrome.notifications.push(
                        "No favorite commands yet. Star a command block to save one.",
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                }
            }
            "Escape" => {
                // Consumed only when a completion menu was actually
                // dismissed; otherwise ESC belongs to the PTY
                // (desktop block_overlay.rs:850-858 returns false).
                let dismissed = self.terminal_blocks.dismiss_completion_menu();
                self.sync_terminal_input_snapshot();
                return dismissed;
            }
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
    /// completion. `entries_json` is `[[name, is_dir], …]`. The same
    /// listing also feeds the terminal link existence cache, so one
    /// daemon round-trip answers both Tab completion and file-link
    /// hover/click validation.
    pub fn terminal_seed_completion_dir(&mut self, dir: &str, entries_json: &str) {
        let Ok(entries) = serde_json::from_str::<Vec<(String, bool)>>(entries_json)
        else {
            return;
        };
        neoism_ui::hint_policy::seed_link_dir_listing(
            PathBuf::from(dir),
            entries.clone(),
        );
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

    /// Toggle `command` in the composer's favorites set — the wasm
    /// half of desktop's star toggle (Cmd+F on a hovered block card,
    /// block_overlay.rs:713-725). Returns `Some(true)` when added,
    /// `Some(false)` when removed, `None` for blank commands. The
    /// favorites feed Ctrl+F's picker; web block cards have no hover
    /// star UI yet, so this is the hook for it.
    pub fn terminal_toggle_favorite_command(&mut self, command: &str) -> Option<bool> {
        self.terminal_blocks.toggle_favorite_command(command)
    }

    /// Insert pasted text into the composer while it owns the line.
    /// Desktop parity: `Screen::paste` routes clipboard text into
    /// `terminal_input.insert_paste` (CRLF-normalised, trailing
    /// newlines trimmed) whenever the block composer is active —
    /// the paste never touches the PTY
    /// (desktop screen/selection/file_link_mouse.rs:349-363).
    pub fn terminal_input_insert_paste(&mut self, text: &str) {
        self.terminal_blocks.insert_paste(text);
        self.sync_terminal_input_snapshot();
    }

    /// Bytes the PTY should receive for pasted text when the
    /// composer does NOT own the line (running command, TUI,
    /// passthrough). Mirrors desktop `Screen::paste`'s PTY branch:
    /// shared `neoism_ui::paste_policy::paste_payload` consulted
    /// with the live BRACKETED_PASTE mode bit, sentinels included
    /// in the bracketed case
    /// (desktop screen/selection/file_link_mouse.rs:365-378).
    pub fn terminal_paste_payload(&mut self, text: &str) -> Vec<u8> {
        use neoism_ui::paste_policy::{
            paste_payload, PastePayload, BRACKETED_PASTE_END, BRACKETED_PASTE_START,
        };
        let bracketed_active = self
            .rendered
            .terminal_ref()
            .inner
            .mode()
            .contains(neoism_terminal_core::crosswords::Mode::BRACKETED_PASTE);
        match paste_payload(text, true, bracketed_active) {
            PastePayload::Bracketed { filtered } => {
                // Desktop re-anchors to the live tail before a
                // bracketed paste (scroll_bottom_when_cursor_not_visible)
                // and clears any selection (Screen::paste's Bracketed
                // arm).
                if self.rendered.terminal_ref().inner.display_offset() != 0 {
                    self.rendered.terminal_mut().inner.scroll_display(
                        neoism_terminal_core::crosswords::grid::Scroll::Bottom,
                    );
                }
                self.terminal_clear_selection_state();
                let mut bytes = Vec::with_capacity(
                    BRACKETED_PASTE_START.len()
                        + filtered.len()
                        + BRACKETED_PASTE_END.len(),
                );
                bytes.extend_from_slice(BRACKETED_PASTE_START);
                bytes.extend_from_slice(&filtered);
                bytes.extend_from_slice(BRACKETED_PASTE_END);
                bytes
            }
            PastePayload::Raw(bytes) => bytes,
        }
    }

    /// The shell kind used to frame submitted command bytes.
    ///
    /// GAP (desktop parity): desktop detects the live foreground
    /// shell per submit (`detect_foreground_shell` over the PTY
    /// fd / pid, block_overlay.rs:917-938). The web/daemon protocol
    /// exposes no shell identity today — `CreatePty` carries an
    /// optional client-CHOSEN shell (the web sends none, so the
    /// daemon falls back to `$SHELL`), and neither `PtyCreated` nor
    /// `ShellHistory` echoes what was actually launched. Until the
    /// protocol carries it, assume zsh — the same assumption the
    /// daemon's history reader makes (`~/.zsh_history` first).
    fn submit_shell_kind(&self) -> TerminalShellKind {
        TerminalShellKind::Zsh
    }

    pub fn terminal_submit_payload(&mut self) -> Vec<u8> {
        let command = self.terminal_blocks.text().to_string();
        let output_start_row = self.terminal_output_start_row();
        let cwd = self.rendered.terminal_ref().inner.current_directory.clone();
        self.terminal_blocks
            .submit_with_context(cwd.as_deref(), output_start_row);
        self.sync_terminal_input_snapshot();
        // The command bytes are about to reach the PTY: re-anchor to
        // the live tail and drop any selection, like every other
        // PTY-bound key (desktop key_event.rs SendToPty arm).
        {
            let terminal = &mut self.rendered.terminal.inner;
            if terminal.display_offset() != 0 {
                terminal.scroll_display(Scroll::Bottom);
            }
        }
        self.terminal_clear_selection_state();
        self.rendered
            .terminal_scroll
            .reset_wheel(TERMINAL_SCROLL_PANE);
        // Desktop threads the live BRACKETED_PASTE bit through to
        // `command_payload` (block_overlay.rs:917); the flag is
        // currently unused there but keeps the call sites identical.
        let bracketed = self
            .rendered
            .terminal_ref()
            .inner
            .mode()
            .contains(neoism_terminal_core::crosswords::Mode::BRACKETED_PASTE);
        self.submit_shell_kind()
            .command_payload(&command, bracketed)
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
        if key != self.active_tab_index {
            // Desktop clears the terminal selection when switching
            // tabs (SelectNextTab / SelectPrevTab → clear_selection).
            self.terminal_clear_selection_state();
            // Link hover + hint mode are terminal-surface state; both
            // reset when the surface goes away (desktop hint mode is
            // per-route and stops on route switch).
            with_terminal_link_ui(|ui| {
                ui.hover = None;
                ui.hint.stop();
            });
        }
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

// ------------------------------------------------------------------
// Terminal grid wheel / pointer / selection surface.
//
// Mirrors the desktop pipeline:
//   * wheel   → Screen::scroll (editor_scroll/editor_command.rs:525-953)
//   * pointer → handle_mouse_input / handle_cursor_moved
//               (app/window_event/mouse.rs) + Screen::on_left_click
//               (screen/selection/file_link_mouse.rs:167-274)
//   * policy  → shared selection_model / mouse_policy / scroll_model
// The JS host feeds logical canvas coordinates; PTY-bound bytes are
// queued and drained via `take_terminal_pointer_bytes`.
// ------------------------------------------------------------------
#[wasm_bindgen]
impl ChromeBridge {
    /// Wheel over the terminal grid. `delta_x` / `delta_y` are PIXELS
    /// in the winit sign convention (positive = scroll up / left; the
    /// JS caller negates DOM deltas). Returns `TERM_INPUT_*` flags;
    /// `0` = not handled, route the wheel to chrome instead.
    pub fn terminal_wheel(
        &mut self,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
        shift: bool,
    ) -> u32 {
        if !self.terminal_surface_active() || !self.terminal_rect_contains(x, y) {
            return 0;
        }
        // Scrolling shifts the row under the cursor; drop the hover
        // underline until the next pointer move re-probes (desktop
        // recomputes the probe per frame instead).
        self.clear_terminal_link_hover();
        let cell_w = self.rendered.cell_w.max(1.0) as f64;
        let cell_h = self.rendered.cell_h.max(1.0) as f64;
        let mode = self.rendered.terminal.inner.mode();

        // Arm 1: a TUI owns the mouse — wheel becomes SGR/legacy
        // wheel reports (codes 64/65), one per accumulated cell.
        if mode.intersects(Mode::MOUSE_MODE) && !mode.contains(Mode::VI) {
            let emit = {
                let p = &mut self.rendered.pointer;
                p.accumulated_scroll_x += delta_x as f64;
                p.accumulated_scroll_y += delta_y as f64;
                TerminalMouseModeWheelReport {
                    accumulated_x: p.accumulated_scroll_x,
                    accumulated_y: p.accumulated_scroll_y,
                    delta_x: delta_x as f64,
                    delta_y: delta_y as f64,
                    width: cell_w,
                    height: cell_h,
                }
                .emit()
            };
            if let Some((pos, _)) = self.terminal_grid_pos_at(x, y, true) {
                for _ in 0..emit.vertical_count {
                    self.queue_terminal_mouse_report(
                        emit.vertical_code,
                        true,
                        pos,
                        false,
                        false,
                        false,
                    );
                }
                for _ in 0..emit.horizontal_count {
                    self.queue_terminal_mouse_report(
                        emit.horizontal_code,
                        true,
                        pos,
                        false,
                        false,
                        false,
                    );
                }
            }
            let p = &mut self.rendered.pointer;
            p.accumulated_scroll_x %= cell_w;
            p.accumulated_scroll_y %= cell_h;
            return TERM_INPUT_HANDLED | self.terminal_pending_pty_flag();
        }

        // Arm 2: alt-screen with alternate-scroll (and no Shift held)
        // — wheel becomes arrow-key CSI so pagers/TUIs scroll.
        if mode.contains(Mode::ALT_SCREEN | Mode::ALTERNATE_SCROLL) && !shift {
            let built = {
                let p = &mut self.rendered.pointer;
                p.accumulated_scroll_x +=
                    (delta_x as f64 * SCROLL_MULTIPLIER) / SCROLL_DIVIDER;
                p.accumulated_scroll_y +=
                    (delta_y as f64 * SCROLL_MULTIPLIER) / SCROLL_DIVIDER;
                TerminalAlternateScrollCsi {
                    accumulated_x: p.accumulated_scroll_x,
                    accumulated_y: p.accumulated_scroll_y,
                    delta_x: delta_x as f64,
                    delta_y: delta_y as f64,
                    width: cell_w,
                    height: cell_h,
                }
                .build()
            };
            if !built.bytes.is_empty() {
                self.rendered.pointer.pending_pty.extend(built.bytes);
            }
            let p = &mut self.rendered.pointer;
            p.accumulated_scroll_x %= cell_w;
            p.accumulated_scroll_y %= cell_h;
            return TERM_INPUT_HANDLED | self.terminal_pending_pty_flag();
        }

        // Arm 3: pixel-perfect scrollback over the shared
        // TerminalScroll (notch accumulation, hard-edge clamping).
        let delta_physical =
            ((delta_y as f64 * SCROLL_MULTIPLIER) / SCROLL_DIVIDER) as f32;
        if delta_physical != 0.0 {
            self.terminal_pixel_scroll(delta_physical);
        }
        TERM_INPUT_HANDLED
    }

    /// Shift+PageUp / Shift+PageDown scrollback paging
    /// (defaults.rs:37-38, `~BindingMode::ALT_SCREEN`). Returns false
    /// on the alt screen so the host falls back to the PTY escape.
    pub fn terminal_scroll_page(&mut self, up: bool) -> bool {
        if !self.terminal_surface_active() {
            return false;
        }
        {
            let terminal = &mut self.rendered.terminal.inner;
            if terminal.mode().contains(Mode::ALT_SCREEN) {
                return false;
            }
            terminal.scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
        }
        self.rendered
            .terminal_scroll
            .reset_wheel(TERMINAL_SCROLL_PANE);
        true
    }

    /// Pointer press on the canvas. Returns `TERM_INPUT_*` flags;
    /// `0` = the press was not for the terminal grid. `button`:
    /// 0 = left, 1 = middle, 2 = right. `now_ms` feeds the desktop
    /// 300ms double/triple click chain.
    #[allow(clippy::too_many_arguments)]
    pub fn terminal_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        button: u8,
        shift: bool,
        ctrl: bool,
        alt: bool,
        now_ms: f64,
    ) -> u32 {
        if !self.terminal_surface_active() || !self.terminal_rect_contains(x, y) {
            return 0;
        }

        {
            let p = &mut self.rendered.pointer;
            match button {
                0 => p.left_pressed = true,
                1 => p.middle_pressed = true,
                2 => p.right_pressed = true,
                _ => {}
            }
            // Desktop click chain (mouse.rs:54-74): same button within
            // 300ms advances Click → Double → Triple; else resets.
            let elapsed = now_ms - p.last_click_ms;
            p.last_click_ms = now_ms;
            p.click_state = if button != p.last_button {
                p.last_button = button;
                1
            } else {
                match p.click_state {
                    1 if elapsed < CLICK_CHAIN_THRESHOLD_MS => 2,
                    2 if elapsed < CLICK_CHAIN_THRESHOLD_MS => 3,
                    _ => 1,
                }
            };
            p.last_x = x;
            p.last_y = y;
        }

        if self.terminal_mouse_mode() && !shift {
            // TUI owns the mouse: report the press, never select
            // (desktop mouse.rs:345-370).
            self.rendered.pointer.click_state = 0;
            let code = match button {
                0 => 0u8,
                1 => 1,
                2 => 2,
                _ => return TERM_INPUT_HANDLED,
            };
            if let Some((pos, _)) = self.terminal_grid_pos_at(x, y, true) {
                self.queue_terminal_mouse_report(code, true, pos, shift, alt, ctrl);
            }
            return TERM_INPUT_HANDLED | self.terminal_pending_pty_flag();
        }

        if button != 0 {
            // Middle/right outside mouse mode: host default behavior
            // (the web has no primary-selection paste).
            return 0;
        }

        let Some((pos, side)) = self.terminal_grid_pos_at(x, y, false) else {
            return 0;
        };

        let click_kind = match self.rendered.pointer.click_state {
            1 => SelectionClickKind::Single,
            2 => SelectionClickKind::Double,
            3 => SelectionClickKind::Triple,
            _ => SelectionClickKind::None,
        };
        let modifiers = SelectionModifiers::new(shift, ctrl, alt);

        // Link click — desktop `Screen::on_left_click`
        // (screen/selection/file_link_mouse.rs:216-242): a plain
        // single click (no Shift/Ctrl/Alt) opens an OSC 8 hyperlink /
        // HTTP(S) link in the browser or a resolvable file/dir token
        // in the editor pane BEFORE any selection starts. Ctrl+click
        // stays a block-selection start, exactly like desktop.
        if should_open_file_link_on_click(click_kind, modifiers) {
            if let Some(open) = self.terminal_link_open_at(pos) {
                with_terminal_link_ui(|ui| ui.pending_opens.push(open));
                self.clear_terminal_link_hover();
                return TERM_INPUT_HANDLED | TERM_INPUT_LINK_OPEN;
            }
        }

        let has_selection = self.rendered.selection_range.is_some();
        match left_click_selection_action(click_kind, modifiers, has_selection, pos, side)
        {
            LeftClickSelectionAction::None => {}
            LeftClickSelectionAction::Extend { point, side } => {
                self.terminal_update_selection_at(point, side);
            }
            LeftClickSelectionAction::Start {
                ty,
                point,
                side,
                clear_existing,
            } => {
                if clear_existing {
                    self.terminal_clear_selection_state();
                }
                self.terminal_start_selection(ty, point, side);
            }
        }
        self.rendered.pointer.selecting = true;
        TERM_INPUT_HANDLED | TERM_INPUT_SELECTING
    }

    /// Pointer move. Drives the in-progress selection drag (clamped
    /// into the grid like desktop's `calculate_mouse_position`) or
    /// mouse-motion reports for TUIs.
    pub fn terminal_pointer_move(
        &mut self,
        x: f32,
        y: f32,
        shift: bool,
        ctrl: bool,
        alt: bool,
    ) -> u32 {
        if !self.terminal_surface_active() {
            return 0;
        }
        let (lmb, mmb, rmb, selecting) = {
            let p = &mut self.rendered.pointer;
            p.last_x = x;
            p.last_y = y;
            (
                p.left_pressed,
                p.middle_pressed,
                p.right_pressed,
                p.selecting,
            )
        };
        let mouse_mode = self.terminal_mouse_mode();

        // Desktop is_selecting gate (mouse.rs:1262-1264).
        if (lmb || rmb) && (shift || !mouse_mode) && selecting {
            if let Some((pos, side)) = self.terminal_grid_pos_at(x, y, true) {
                self.terminal_update_selection_at(pos, side);
                return TERM_INPUT_HANDLED;
            }
            return 0;
        }

        if !self.terminal_rect_contains(x, y) {
            return 0;
        }
        let mode = self.rendered.terminal.inner.mode();
        if mouse_mode && mode.intersects(Mode::MOUSE_MOTION | Mode::MOUSE_DRAG) {
            let code = if lmb {
                32u8
            } else if mmb {
                33
            } else if rmb {
                34
            } else if mode.intersects(Mode::MOUSE_MOTION) {
                35
            } else {
                return 0;
            };
            let Some((pos, _)) = self.terminal_grid_pos_at(x, y, true) else {
                return 0;
            };
            // Desktop's square_changed gate: one report per cell.
            let cell = (pos.row.0, pos.col.0);
            if self.rendered.pointer.last_report_cell == Some(cell) {
                return 0;
            }
            self.rendered.pointer.last_report_cell = Some(cell);
            self.queue_terminal_mouse_report(code, true, pos, shift, alt, ctrl);
            return TERM_INPUT_HANDLED | self.terminal_pending_pty_flag();
        }
        0
    }

    /// Pointer release. Ends the drag / reports the release in mouse
    /// mode. Always safe to call (also resets button state when the
    /// press landed elsewhere).
    pub fn terminal_pointer_up(
        &mut self,
        x: f32,
        y: f32,
        button: u8,
        shift: bool,
        ctrl: bool,
        alt: bool,
    ) -> u32 {
        let was_selecting = {
            let p = &mut self.rendered.pointer;
            match button {
                0 => p.left_pressed = false,
                1 => p.middle_pressed = false,
                2 => p.right_pressed = false,
                _ => {}
            }
            p.last_report_cell = None;
            let was = p.selecting && button == 0;
            if button == 0 {
                p.selecting = false;
            }
            was
        };
        if !self.terminal_surface_active() {
            return 0;
        }
        if self.terminal_mouse_mode() && !shift && !was_selecting {
            let code = match button {
                0 => 0u8,
                1 => 1,
                2 => 2,
                _ => return 0,
            };
            if let Some((pos, _)) = self.terminal_grid_pos_at(x, y, true) {
                self.queue_terminal_mouse_report(code, false, pos, shift, alt, ctrl);
            }
            return TERM_INPUT_HANDLED | self.terminal_pending_pty_flag();
        }
        // Desktop's copy_on_select is default-off; release just ends
        // the drag.
        if was_selecting {
            TERM_INPUT_HANDLED
        } else {
            0
        }
    }

    /// One 15ms autoscroll tick while a selection drag sits in the
    /// top/bottom edge zone (desktop `selection_scroll_tick` +
    /// `selection_scroll_pixels`). Returns true when a redraw is due.
    pub fn terminal_drag_scroll_tick(&mut self) -> bool {
        if !self.terminal_surface_active() {
            return false;
        }
        let (lmb, selecting, last_x, last_y) = {
            let p = &self.rendered.pointer;
            (p.left_pressed, p.selecting, p.last_x, p.last_y)
        };
        if !lmb || !selecting || self.rendered.selection_range.is_none() {
            return false;
        }
        let rect = self.chrome.layout().terminal;
        let cell_h = self.rendered.cell_h.max(1.0) as f64;
        let top = rect.y as f64;
        let bottom = (rect.y + rect.h) as f64;
        let edge_zone = (cell_h * 2.5).max(32.0);
        let y = last_y as f64;
        // Speed ramps from 0.35 to 1.7 cell-heights per tick across
        // the edge zone, exactly like desktop.
        let delta = if y < top + edge_zone {
            let t = ((top + edge_zone - y).max(0.0) / edge_zone).clamp(0.0, 1.0);
            cell_h * (0.35 + t * 1.35)
        } else if y > bottom - edge_zone {
            let t = ((y - (bottom - edge_zone)).max(0.0) / edge_zone).clamp(0.0, 1.0);
            -cell_h * (0.35 + t * 1.35)
        } else {
            0.0
        };
        if delta == 0.0 {
            return false;
        }
        // Desktop routes the tick through Screen::scroll, which folds
        // in the same wheel multiplier before the notch accumulator.
        self.terminal_pixel_scroll(((delta * SCROLL_MULTIPLIER) / SCROLL_DIVIDER) as f32);
        if let Some((pos, side)) = self.terminal_grid_pos_at(last_x, last_y, true) {
            self.terminal_update_selection_at(pos, side);
        }
        true
    }

    /// Text of the current selection (`selection_model::selected_text`
    /// over the live grid — same call desktop's copy path makes).
    pub fn terminal_selected_text(&self) -> Option<String> {
        selected_text(&self.rendered.terminal.inner, self.rendered.selection_range)
    }

    pub fn terminal_has_selection(&self) -> bool {
        self.rendered.selection_range.is_some()
    }

    pub fn terminal_clear_selection(&mut self) {
        self.terminal_clear_selection_state();
    }

    /// Drain queued PTY-bound mouse-report / CSI bytes.
    pub fn take_terminal_pointer_bytes(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rendered.pointer.pending_pty)
    }

    /// Key bytes are about to reach the PTY: desktop's SendToPty arm
    /// (key_event.rs:684-697) snaps scrollback to the live tail and
    /// clears any selection. Returns true when something changed.
    pub fn terminal_notify_key_input(&mut self) -> bool {
        if !self.terminal_surface_active() {
            return false;
        }
        let mut changed = false;
        {
            let terminal = &mut self.rendered.terminal.inner;
            if terminal.display_offset() != 0 {
                terminal.scroll_display(Scroll::Bottom);
                changed = true;
            }
        }
        if self.rendered.selection_range.is_some()
            || self.rendered.terminal.inner.selection.is_some()
        {
            self.terminal_clear_selection_state();
            changed = true;
        }
        self.rendered
            .terminal_scroll
            .reset_wheel(TERMINAL_SCROLL_PANE);
        changed
    }

    // --------------------------------------------------------------
    // PTY key-byte encoding export (desktop parity).
    //
    // Mirrors the desktop pipeline exactly: bindings/defaults.rs
    // Action::Esc table (DECCKM SS3 arrows, tildes, Backspace family,
    // F1-F4 SS3, Shift+Tab) → alt masking (`Screen::alt_send_esc`) →
    // `should_build_key_sequence` fork between the kitty/CSI builder
    // and the raw-UTF-8 path. The decision logic lives in
    // `neoism_ui::key_policy::web_key_encoder` (native-testable); this
    // export feeds it the live terminal mode bits the way desktop's
    // `Screen::get_mode()` snapshot does.
    // --------------------------------------------------------------

    /// Encode one pressed/repeated key into the PTY byte sequence the
    /// desktop build would produce for the same key in the same
    /// terminal mode. Arguments mirror the DOM `KeyboardEvent` fields
    /// (`key`, `code`, modifier booleans, `repeat`); `meta` is
    /// `event.metaKey` (Super). Returns an empty array when the key is
    /// not PTY-bound in the current mode (no representation, or the
    /// desktop pipeline consumes it host-side: scrollback paging, tab
    /// management, chrome focus, copy/paste, font size, vi mode).
    #[allow(clippy::too_many_arguments)]
    pub fn encode_terminal_key(
        &self,
        key: &str,
        code: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
        repeat: bool,
    ) -> Vec<u8> {
        use neoism_ui::key_policy::web_key_encoder::{
            encode_web_terminal_key, WebTerminalKeyInput, WebTerminalKeyModes,
        };
        let mode = self.rendered.terminal_ref().inner.mode();
        let modes = WebTerminalKeyModes {
            app_cursor: mode.contains(Mode::APP_CURSOR),
            alt_screen: mode.contains(Mode::ALT_SCREEN),
            vi: mode.contains(Mode::VI),
            disambiguate_esc_codes: mode.contains(Mode::DISAMBIGUATE_ESC_CODES),
            report_event_types: mode.contains(Mode::REPORT_EVENT_TYPES),
            report_alternate_keys: mode.contains(Mode::REPORT_ALTERNATE_KEYS),
            report_all_keys_as_esc: mode.contains(Mode::REPORT_ALL_KEYS_AS_ESC),
            report_associated_text: mode.contains(Mode::REPORT_ASSOCIATED_TEXT),
        };
        encode_web_terminal_key(
            &WebTerminalKeyInput {
                key,
                code,
                ctrl,
                alt,
                shift,
                super_key: meta,
                repeat,
            },
            &modes,
        )
    }
}

impl ChromeBridge {
    pub(crate) fn terminal_surface_active(&self) -> bool {
        self.chrome.is_terminal_tab_active() && !self.chrome.is_neoism_agent_tab_active()
    }

    fn terminal_rect_contains(&self, x: f32, y: f32) -> bool {
        let rect = self.chrome.layout().terminal;
        x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
    }

    fn terminal_mouse_mode(&self) -> bool {
        // Desktop `Screen::mouse_mode` (selection/mouse_position.rs).
        let mode = self.rendered.terminal.inner.mode();
        mode.intersects(Mode::MOUSE_MODE) && !mode.contains(Mode::VI)
    }

    fn terminal_pending_pty_flag(&self) -> u32 {
        if self.rendered.pointer.pending_pty.is_empty() {
            0
        } else {
            TERM_INPUT_HAS_PTY
        }
    }

    /// Map a canvas point to a terminal `Pos` (absolute `Line`
    /// anchoring, so selections survive scrollback movement) + the
    /// half-cell `Side`. With `clamp` the point is clamped into the
    /// grid (drag semantics); without it, points outside the terminal
    /// rect return `None`. Under the block pipeline the composed
    /// visual-row → source-row map resolves rows; injected block
    /// chrome rows return `None` (desktop `Some(None) → None`).
    fn terminal_grid_pos_at(&self, x: f32, y: f32, clamp: bool) -> Option<(Pos, Side)> {
        let rect = self.chrome.layout().terminal;
        if !clamp && !self.terminal_rect_contains(x, y) {
            return None;
        }
        let cell_w = self.rendered.cell_w.max(1.0);
        let cell_h = self.rendered.cell_h.max(1.0);
        let terminal = &self.rendered.terminal.inner;
        let cols = terminal.columns().max(1);
        let rows = terminal.screen_lines().max(1);
        let col =
            (((x - rect.x) / cell_w).floor() as i64).clamp(0, cols as i64 - 1) as usize;
        let visual_row =
            (((y - rect.y) / cell_h).floor() as i64).clamp(0, rows as i64 - 1) as usize;
        let side = cell_side_by_pos((x - rect.x).max(0.0) as usize, 0.0, cell_w, rect.w);
        let line = if let Some(sources) = self.rendered.pointer.frame_sources.as_ref() {
            match sources.get(visual_row).copied() {
                Some(Some(abs)) => {
                    let line = abs as i64 - terminal.history_size() as i64;
                    Line(line.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
                }
                _ => return None,
            }
        } else {
            Line(visual_row as i32 - terminal.display_offset() as i32)
        };
        Some((Pos::new(line, Column(col)), side))
    }

    /// Queue one SGR / legacy mouse report for the PTY. Mirrors
    /// desktop `Screen::mouse_report`
    /// (selection/file_link_mouse.rs:291-327).
    fn queue_terminal_mouse_report(
        &mut self,
        button: u8,
        pressed: bool,
        pos: Pos,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) {
        // Never report positions in scrollback.
        if pos.row.0 < 0 {
            return;
        }
        let mode = self.rendered.terminal.inner.mode();
        let mods = mouse_report_modifier_bits(shift, alt, ctrl);
        if mode.contains(Mode::SGR_MOUSE) {
            self.rendered
                .pointer
                .pending_pty
                .extend(encode_sgr_mouse_report(pos, button + mods, pressed));
        } else {
            let byte = mouse_report_legacy_button_byte(button, mods, pressed);
            if let Some(msg) =
                encode_normal_mouse_report(pos, byte, mode.contains(Mode::UTF8_MOUSE))
            {
                self.rendered.pointer.pending_pty.extend(msg);
            }
        }
    }

    /// Desktop `Screen::scroll`'s raw terminal arm
    /// (editor_command.rs:571-585, 675-728, 926-944): notch
    /// accumulation via the shared `TerminalScroll`, hard-edge
    /// clamping via `reset_wheel`. `delta_physical` is in physical
    /// pixels, winit sign. Returns true when the display offset moved.
    fn terminal_pixel_scroll(&mut self, delta_physical: f32) -> bool {
        let cell_h = self.rendered.cell_h.max(1.0);
        let (display_offset, history_size) = {
            let terminal = &self.rendered.terminal.inner;
            (terminal.display_offset(), terminal.history_size())
        };
        // Hard edge: rejected wheel input must not leave the content
        // parked between rows.
        let edge_rejected = (delta_physical > 0.0 && display_offset >= history_size)
            || (delta_physical < 0.0 && display_offset == 0);
        if edge_rejected {
            self.rendered
                .terminal_scroll
                .reset_wheel(TERMINAL_SCROLL_PANE);
            return false;
        }
        let committed = self.rendered.terminal_scroll.add_wheel_delta(
            TERMINAL_SCROLL_PANE,
            delta_physical,
            cell_h,
        );
        if committed == 0 {
            return false;
        }
        let moved = {
            let terminal = &mut self.rendered.terminal.inner;
            let old = terminal.display_offset();
            terminal.scroll_display(Scroll::Delta(committed));
            terminal.display_offset() != old
        };
        if !moved {
            // Hit the hard edge mid-commit; clear the residual so the
            // next wheel input starts cleanly.
            self.rendered
                .terminal_scroll
                .reset_wheel(TERMINAL_SCROLL_PANE);
        }
        moved
    }

    /// Start a selection of `ty` at `point` — desktop
    /// `Screen::start_selection` minus the primary-selection copy
    /// (the web has no primary clipboard).
    fn terminal_start_selection(&mut self, ty: SelectionType, point: Pos, side: Side) {
        let terminal = &mut self.rendered.terminal.inner;
        let (selection, range) =
            selection_with_range(terminal, ty, SelectionEndpoint::new(point, side), None);
        terminal.selection = Some(selection);
        self.rendered.selection_range = range;
    }

    /// Desktop `Screen::update_selection` over the shared
    /// `apply_selection_update` (keeps the previous painted range
    /// when the update yields an empty one, like desktop).
    fn terminal_update_selection_at(&mut self, point: Pos, side: Side) {
        let terminal = &mut self.rendered.terminal.inner;
        let vi_mode = terminal.mode().contains(Mode::VI);
        if let Some(range) = apply_selection_update(terminal, point, side, vi_mode, false)
        {
            self.rendered.selection_range = Some(range);
        }
    }

    pub(crate) fn terminal_clear_selection_state(&mut self) {
        self.rendered.terminal.inner.selection = None;
        self.rendered.selection_range = None;
    }
}

// ------------------------------------------------------------------
// Terminal link surface: hover underline, plain-click open, hint mode.
//
// Web port of desktop screen/selection/file_link_mouse.rs +
// terminal/hints.rs over the SHARED machinery: probes from
// `selection_model` (`terminal_file_link_probe` /
// `terminal_wrapped_link_probe` / `hyperlink_span_at`), token +
// existence policy from `hint_policy`, and the hint-mode state
// machine from `hint_state`. Existence checks are daemon-seeded dir
// listings (the wasm has no filesystem), drained by the JS host and
// answered through `terminal_seed_completion_dir`.
//
// State lives in a thread_local because `ChromeBridge` cannot grow
// fields from this module — the same pattern `palettes_finder` and
// `editor_panes` already use.
// ------------------------------------------------------------------

/// Hovered link span (visual grid coordinates in the terminal rect).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalLinkHover {
    pub(crate) visual_row: usize,
    pub(crate) col_start: usize,
    pub(crate) col_end: usize,
}

/// One queued "open this link" intent for the JS host.
/// `kind`: `"url"` → window.open, `"file"` → editor open (with an
/// optional 1-based `line` jump), `"dir"` → file-tree reveal.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct TerminalLinkOpen {
    pub(crate) kind: &'static str,
    pub(crate) target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<u32>,
}

pub(crate) struct TerminalLinkUiState {
    pub(crate) hover: Option<TerminalLinkHover>,
    pub(crate) hint: HintState<SimpleHintConfig>,
    pub(crate) pending_opens: Vec<TerminalLinkOpen>,
    /// Parent dirs the existence probe wants listed; JS drains these
    /// and answers via `terminal_seed_completion_dir`.
    pub(crate) pending_dir_requests: Vec<String>,
}

impl Default for TerminalLinkUiState {
    fn default() -> Self {
        Self {
            hover: None,
            hint: HintState::new(
                neoism_ui::hint_policy::DEFAULT_HINT_ALPHABET.to_string(),
            ),
            pending_opens: Vec::new(),
            pending_dir_requests: Vec::new(),
        }
    }
}

thread_local! {
    static TERMINAL_LINK_UI: std::cell::RefCell<TerminalLinkUiState> =
        std::cell::RefCell::new(TerminalLinkUiState::default());
}

pub(crate) fn with_terminal_link_ui<R>(
    f: impl FnOnce(&mut TerminalLinkUiState) -> R,
) -> R {
    TERMINAL_LINK_UI.with(|cell| f(&mut cell.borrow_mut()))
}

/// The `regex_finder` handed to the shared `HintState`: the desktop
/// compiles `DEFAULT_URL_REGEX` through oniguruma, which doesn't build
/// on wasm — the shared `web_hint_line_spans` scan approximates the
/// same three branches (schemed URLs, rooted/explicit paths, bare
/// relative paths with an extension).
fn web_hint_regex_finder(line: &str, _pattern: &str) -> Vec<(usize, usize)> {
    web_hint_line_spans(line)
}

/// Hover / click flag bits shared with the JS host.
const TERM_LINK_CHANGED: u32 = 1;
const TERM_LINK_HOVERING: u32 = 2;
const TERM_LINK_PENDING_DIRS: u32 = 4;
/// `terminal_hint_key` flag bits.
const TERM_HINT_CONSUMED: u32 = 1;
const TERM_HINT_OPEN: u32 = 2;

#[wasm_bindgen]
impl ChromeBridge {
    /// Probe the terminal grid under the pointer for a clickable link
    /// (OSC 8 hyperlink → wrapped web link → file token) and update
    /// the hover-underline span. Returns `TERM_LINK_*` bits: 1 = the
    /// hover changed (redraw), 2 = a link is under the pointer,
    /// 4 = existence lookups queued dir-listing requests (drain
    /// `terminal_drain_link_dir_requests`).
    pub fn terminal_hover_probe(&mut self, x: f32, y: f32) -> u32 {
        let mut new_hover = None;
        if self.terminal_surface_active()
            && self.terminal_rect_contains(x, y)
            && !self.rendered.pointer.selecting
            && !self.terminal_mouse_mode()
            && !with_terminal_link_ui(|ui| ui.hint.is_active())
        {
            if let Some((pos, _)) = self.terminal_grid_pos_at(x, y, false) {
                let rect = self.chrome.layout().terminal;
                let cell_h = self.rendered.cell_h.max(1.0);
                let visual_row = (((y - rect.y) / cell_h).floor() as i64).max(0) as usize;
                new_hover =
                    self.terminal_link_cols_at(pos).map(|(col_start, col_end)| {
                        TerminalLinkHover {
                            visual_row,
                            col_start,
                            col_end,
                        }
                    });
            }
        }
        let hovering = new_hover.is_some();
        let (changed, pending_dirs) = with_terminal_link_ui(|ui| {
            let changed = ui.hover != new_hover;
            ui.hover = new_hover;
            for dir in neoism_ui::hint_policy::drain_link_dir_requests() {
                ui.pending_dir_requests
                    .push(dir.to_string_lossy().into_owned());
            }
            (changed, !ui.pending_dir_requests.is_empty())
        });
        let mut flags = 0;
        if changed {
            flags |= TERM_LINK_CHANGED;
        }
        if hovering {
            flags |= TERM_LINK_HOVERING;
        }
        if pending_dirs {
            flags |= TERM_LINK_PENDING_DIRS;
        }
        flags
    }

    /// Queued link-open intents (`[{kind, target, line?}]`), drained
    /// after a link click (`TERM_INPUT_LINK_OPEN`) or a hint fire.
    pub fn terminal_drain_link_opens(&mut self) -> JsValue {
        let opens = with_terminal_link_ui(|ui| std::mem::take(&mut ui.pending_opens));
        serde_wasm_bindgen::to_value(&opens).unwrap_or(JsValue::NULL)
    }

    /// Parent directories the link existence probe wants listed. JS
    /// fetches each through the daemon Files surface and seeds the
    /// answer back via `terminal_seed_completion_dir`.
    pub fn terminal_drain_link_dir_requests(&mut self) -> JsValue {
        let dirs =
            with_terminal_link_ui(|ui| std::mem::take(&mut ui.pending_dir_requests));
        serde_wasm_bindgen::to_value(&dirs).unwrap_or(JsValue::NULL)
    }

    /// Land a deferred `file:line` jump once the opened file's pane is
    /// live. Mirrors the chrome-side `:N` ex jump
    /// (palettes_finder.rs `execute_ex_command_chrome_side`): code
    /// pane cursor + follow, markdown `jump_to_line`. Returns false
    /// while no pane is live yet — the host retries briefly.
    pub fn terminal_link_goto_line(&mut self, line: u32) -> bool {
        use neoism_ui::editor::code::{CodeInputMode, CodeMode};
        let line = (line as usize).max(1);
        if let Some(pane) = self.chrome.code_pane_mut() {
            let line_ix = (line - 1).min(pane.buffer.lines.len().saturating_sub(1));
            pane.buffer.set_cursor_position(line_ix, 0, false);
            pane.buffer.follow_cursor = true;
            if pane.input_mode == CodeInputMode::Vim
                && pane.buffer.mode == CodeMode::Normal
            {
                pane.buffer.snap_normal_cursor();
            }
            return true;
        }
        if let Some(pane) = self.chrome.markdown_pane_mut() {
            pane.jump_to_line(line);
            return true;
        }
        false
    }

    /// Enter hint mode over the visible grid — the web wiring of the
    /// desktop default hint binding (bindings/defaults.rs
    /// `create_hint_bindings`, Ctrl+Shift+O): label every visible
    /// URL / path / OSC 8 hyperlink and open on keystroke narrowing.
    /// Returns whether hint mode is active (desktop cancels silently
    /// when nothing matches).
    pub fn terminal_hint_start(&mut self) -> bool {
        if !self.terminal_surface_active() {
            return false;
        }
        let config = std::rc::Rc::new(SimpleHintConfig {
            // Marker only: `web_hint_regex_finder` ignores the pattern
            // and runs the shared regex-free scan.
            regex: Some("<web-default-url>".to_string()),
            hyperlinks: true,
            post_processing: true,
            persist: false,
        });
        let terminal = &self.rendered.terminal.inner;
        with_terminal_link_ui(|ui| {
            ui.hover = None;
            ui.hint.start(config);
            ui.hint.update_matches(terminal, web_hint_regex_finder);
            ui.hint.is_active()
        })
    }

    /// Whether hint mode currently owns the keyboard.
    pub fn terminal_hint_active(&self) -> bool {
        with_terminal_link_ui(|ui| ui.hint.is_active())
    }

    /// Route one keydown into hint mode. Key mapping mirrors desktop
    /// selection/key_event.rs:399-483 via the shared classifier:
    /// Escape stops, Backspace pops, printable chars narrow labels,
    /// everything else is swallowed (all bindings are disabled while
    /// a hint is being selected, like Alacritty). Returns
    /// `TERM_HINT_*` bits: 1 = consumed (always, while active),
    /// 2 = a match fired and open intents are queued.
    pub fn terminal_hint_key(&mut self, key: &str) -> u32 {
        if !with_terminal_link_ui(|ui| ui.hint.is_active()) {
            return 0;
        }
        let chars: Vec<char> = match key {
            "Escape" => vec!['\u{1b}'],
            "Backspace" => vec!['\u{8}'],
            k if k.chars().count() == 1 => k.chars().collect(),
            // Named/modifier keys: consumed, nothing fed (desktop
            // feeds the key event's empty text).
            _ => Vec::new(),
        };
        let mut fired = false;
        for c in chars {
            let hint_match = {
                let terminal = &self.rendered.terminal.inner;
                with_terminal_link_ui(|ui| {
                    ui.hint.keyboard_input(terminal, c, web_hint_regex_finder)
                })
            };
            if let Some(hint_match) = hint_match {
                let open = self.terminal_link_open_for_text(&hint_match.text);
                with_terminal_link_ui(|ui| ui.pending_opens.push(open));
                fired = true;
            }
        }
        TERM_HINT_CONSUMED | if fired { TERM_HINT_OPEN } else { 0 }
    }
}

impl ChromeBridge {
    pub(crate) fn clear_terminal_link_hover(&mut self) {
        with_terminal_link_ui(|ui| ui.hover = None);
    }

    /// The terminal's OSC 7 cwd, falling back to the workspace root —
    /// the web stand-in for desktop
    /// `current_terminal_completion_cwd`.
    fn terminal_link_cwd(&self) -> PathBuf {
        self.rendered
            .terminal_ref()
            .inner
            .current_directory
            .clone()
            .unwrap_or_else(|| self.workspace_root.clone())
    }

    /// Resolve a link token to an absolute candidate path. Mirrors
    /// desktop `terminal::file_link::resolve_token` minus `~/` (the
    /// daemon home dir isn't known client-side) and minus the
    /// existence check (that's `link_path_existence`).
    fn resolve_terminal_link_path(&self, token: &str) -> Option<PathBuf> {
        if token.is_empty() || hint_text_is_url(token) || token.contains("://") {
            return None;
        }
        if token.starts_with('/') {
            return Some(PathBuf::from(token));
        }
        if token.starts_with('~') || token.starts_with('$') {
            // Home / env expansion needs host state the web lacks.
            return None;
        }
        Some(self.terminal_link_cwd().join(token))
    }

    /// Column span of the link under `pos`, if any — the hover
    /// underline geometry. Probe order matches the desktop hover
    /// (`draw_terminal_file_link_hover`): web link before file token,
    /// with OSC 8 hyperlinks checked first (the wasm cell path never
    /// carried hyperlink ids, so this is the web's OSC 8 debut).
    fn terminal_link_cols_at(&self, pos: Pos) -> Option<(usize, usize)> {
        let terminal = &self.rendered.terminal_ref().inner;
        if let Some(span) = hyperlink_span_at(terminal, pos, true) {
            return Some((span.start.col.0, span.end.col.0 + 1));
        }
        if let Some(probe) = terminal_wrapped_link_probe(terminal, pos) {
            if let Some(link) = detect_web_link_in_wrapped_row(
                &probe.row_text,
                probe.col,
                probe.physical_row_start,
                probe.physical_row_len,
            ) {
                return Some((link.col_start, link.col_end));
            }
        }
        let probe = terminal_file_link_probe(terminal, pos)?;
        let token = terminal_file_link_token_at(&probe.row_text, probe.col)?;
        let (path_part, _) = split_file_link_line_suffix(&token.text);
        let resolved = self.resolve_terminal_link_path(path_part)?;
        match link_path_existence(&resolved) {
            LinkPathExistence::Exists { .. } => Some((token.col_start, token.col_end)),
            _ => None,
        }
    }

    /// The open intent for a plain click at `pos` — desktop
    /// `on_left_click`'s link arm: web link → browser, file token →
    /// editor / markdown / file-tree by `file_link_open_target`.
    fn terminal_link_open_at(&mut self, pos: Pos) -> Option<TerminalLinkOpen> {
        let terminal = &self.rendered.terminal_ref().inner;
        if let Some(span) = hyperlink_span_at(terminal, pos, true) {
            return Some(TerminalLinkOpen {
                kind: "url",
                target: span.uri,
                line: None,
            });
        }
        if let Some(probe) = terminal_wrapped_link_probe(terminal, pos) {
            if let Some(link) = detect_web_link_in_wrapped_row(
                &probe.row_text,
                probe.col,
                probe.physical_row_start,
                probe.physical_row_len,
            ) {
                return Some(TerminalLinkOpen {
                    kind: "url",
                    target: link.url,
                    line: None,
                });
            }
        }
        let probe = terminal_file_link_probe(terminal, pos)?;
        let token = terminal_file_link_token_at(&probe.row_text, probe.col)?;
        let (path_part, line) = split_file_link_line_suffix(&token.text);
        let resolved = self.resolve_terminal_link_path(path_part)?;
        match link_path_existence(&resolved) {
            LinkPathExistence::Exists { is_dir } => Some(TerminalLinkOpen {
                kind: if is_dir { "dir" } else { "file" },
                target: resolved.to_string_lossy().into_owned(),
                line: if is_dir { None } else { line },
            }),
            _ => None,
        }
    }

    /// Convert a fired hint match's text into an open intent. Desktop
    /// hands the text to `xdg-open`; the web opens URLs in a new tab
    /// and paths in the editor pane. Unresolvable tokens still open
    /// as files with the raw text (the user explicitly picked the
    /// label), letting the daemon report a miss.
    fn terminal_link_open_for_text(&self, text: &str) -> TerminalLinkOpen {
        if hint_text_is_url(text) {
            return TerminalLinkOpen {
                kind: "url",
                target: text.to_string(),
                line: None,
            };
        }
        let (path_part, line) = split_file_link_line_suffix(text);
        let target = self
            .resolve_terminal_link_path(path_part)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_part.to_string());
        let is_dir = matches!(
            link_path_existence(Path::new(&target)),
            LinkPathExistence::Exists { is_dir: true }
        );
        TerminalLinkOpen {
            kind: if is_dir { "dir" } else { "file" },
            target,
            line: if is_dir { None } else { line },
        }
    }
}
