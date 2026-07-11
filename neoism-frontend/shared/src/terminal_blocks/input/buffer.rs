use super::*;

use neoism_terminal_core::crosswords::ShellPromptState;
use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::OpenOptions;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::path::{Path, PathBuf};
use web_time::Duration;
use web_time::Instant;

use crate::input::CompletionFlashState;

impl TerminalInputBuffer {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn has_visible_footer(&self, show_input: bool) -> bool {
        show_input || !self.command_blocks.is_empty() || !self.completion_items.is_empty()
    }

    pub fn composer_footer_active(
        &self,
        state: ShellPromptState,
        terminal_alt_screen: bool,
        _unused: bool,
    ) -> bool {
        // Hide the composer chassis (and the layout space it reserves)
        // whenever the terminal is fully owned by another program:
        //   - alt-screen TUI (vim, htop, less, btop, …) needs the
        //     full grid for its own UI.
        //   - Passthrough sessions (sh, ssh, mosh) bypass our shell
        //     hooks so the composer would render stale state.
        //   - A foreground command is actively running, which means
        //     the user is interacting with that CLI directly and any
        //     command-bar chrome would collide with its output.
        if terminal_alt_screen || self.passthrough_session_active || state.running_command
        {
            return false;
        }

        self.has_visible_footer(self.editing_window_open(state) && !terminal_alt_screen)
    }

    /// Whether the composer — not the raw PTY — should own the next
    /// printable keystroke. This is the single source of truth both the
    /// desktop and web frontends route through, so the displayed
    /// command and the bytes sent on Enter can never come from two
    /// different sinks.
    ///
    /// The naive gate ("`awaiting_command` is true") flips mid-command
    /// in two ways that diverge typed input from what runs:
    ///
    ///   1. Fresh terminal: before the shell prints its first prompt
    ///      (OSC 133;B) `awaiting_command` is `false`, so the first
    ///      keystrokes leak to the shell's own line editor; the rest,
    ///      after the prompt latches, land in the composer. On Enter the
    ///      composer submits only its half. `editing_window_open` keeps
    ///      the boot window (`!ever_awaited_command`, nothing running)
    ///      on the composer side.
    ///   2. Mid-command repaint: once the composer holds pending text it
    ///      must never relinquish until submit/clear, even if a stray
    ///      state read briefly shows `awaiting_command == false`. The
    ///      `!self.text.is_empty()` arm pins ownership.
    pub fn should_capture_input(
        &self,
        state: ShellPromptState,
        terminal_alt_screen: bool,
    ) -> bool {
        if terminal_alt_screen || self.passthrough_session_active {
            return false;
        }
        // A foreground command owns the terminal; its keystrokes belong
        // to that process — unless the composer is already mid-edit
        // (pending text), in which case the user is composing the next
        // command and we must not split it across two sinks.
        if state.running_command {
            return !self.text.is_empty();
        }
        self.editing_window_open(state) || !self.text.is_empty()
    }

    /// True while the shell is at an editable prompt — either it has
    /// reported `awaiting_command`, or we are in the fresh-terminal boot
    /// window before the first prompt latched (`!ever_awaited_command`)
    /// with no command running. Callers must still exclude alt-screen /
    /// passthrough / running-command states themselves. Exposed so the
    /// layout-reservation paths (`host::run`, `host::composer`) reserve
    /// the composer gap on exactly the frames the composer is shown.
    pub fn editing_window_open(&self, state: ShellPromptState) -> bool {
        state.awaiting_command || (!self.ever_awaited_command && !state.running_command)
    }

    pub fn is_prompt_animating(&self) -> bool {
        if !self.completion_items.is_empty() {
            return true;
        }
        if self.control_notice().is_some() {
            return true;
        }
        if self.prompt_burst_elapsed_ms_internal().is_some() {
            return true;
        }
        if self.flash_state().is_some() {
            return true;
        }
        // Running blocks drive the spinner — keep redraw-ticking until
        // every block has settled to Finished, otherwise the spinner
        // freezes on its first frame and looks like a stuck command.
        self.command_blocks
            .iter()
            .any(|block| matches!(block.status, TerminalCommandBlockStatus::Running))
    }

    pub fn passthrough_session_active(&self) -> bool {
        self.passthrough_session_active
    }

    /// Number of command blocks currently tracked. The splash
    /// dismiss trigger reads this — > 0 means the user has
    /// submitted at least one non-empty command since the pane
    /// started, regardless of whether its output has scrolled
    /// far enough to push the splash up out of view.
    pub fn command_block_count(&self) -> usize {
        self.command_blocks.len()
    }

    pub fn set_passthrough_session_active(&mut self, active: bool) {
        self.passthrough_session_active = active;
    }

    /// Bind a `neoism`-format history file (NUL-delimited entries) to
    /// this buffer. Subsequent `submit_with_context` calls append to
    /// the same file. The desktop fork's `enable_persistent_history_
    /// default` resolves the path with `dirs::data_local_dir()` and
    /// then calls this. Web frontends supply a host-managed path
    /// instead (or skip persistence entirely).
    pub fn enable_persistent_history(&mut self, path: PathBuf) {
        self.persistent_history = Some(PersistentHistory::Neoism(path.clone()));
        self.load_neoism_history(&path);
    }

    pub fn enable_persistent_favorites(&mut self, path: PathBuf) {
        self.persistent_favorites = Some(path.clone());
        self.load_favorites(&path);
    }

    /// Bind a zsh `HISTFILE` to this buffer. Loads the file for
    /// suggestions but does **not** append on submit — zsh itself owns
    /// HISTFILE writes and duplicating entries here would make the
    /// native history noisy.
    pub fn enable_zsh_history(&mut self, path: PathBuf) {
        self.persistent_history = Some(PersistentHistory::Zsh(path.clone()));
        self.load_zsh_history(&path);
    }

    /// Replace the in-memory history with `entries`. Used by frontends
    /// that have already resolved the persistent history out-of-band
    /// (e.g. the web frontend, which receives entries from the daemon
    /// instead of touching the filesystem itself).
    pub fn set_history(&mut self, entries: Vec<String>) {
        let mut entries = entries;
        if entries.len() > HISTORY_LIMIT {
            let keep_from = entries.len() - HISTORY_LIMIT;
            entries.drain(0..keep_from);
        }
        self.history = entries;
    }

    pub(crate) fn load_neoism_history(&mut self, path: &Path) {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let mut loaded = bytes
            .split(|byte| *byte == 0)
            .filter_map(|chunk| {
                if chunk.is_empty() {
                    return None;
                }
                String::from_utf8(chunk.to_vec())
                    .ok()
                    .map(|entry| sanitize_history_entry(&entry))
                    .filter(|entry| !entry.trim().is_empty())
            })
            .collect::<Vec<_>>();
        if loaded.len() > HISTORY_LIMIT {
            let keep_from = loaded.len() - HISTORY_LIMIT;
            loaded.drain(0..keep_from);
        }
        self.history = loaded;
    }

    pub(crate) fn load_zsh_history(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let mut loaded = text
            .lines()
            .filter_map(parse_zsh_history_line)
            .collect::<Vec<_>>();
        if loaded.len() > HISTORY_LIMIT {
            let keep_from = loaded.len() - HISTORY_LIMIT;
            loaded.drain(0..keep_from);
        }
        self.history = loaded;
    }

    pub(crate) fn load_favorites(&mut self, path: &Path) {
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let mut seen = BTreeSet::new();
        let mut loaded = Vec::new();
        for chunk in bytes.split(|byte| *byte == 0) {
            if chunk.is_empty() {
                continue;
            }
            let Some(command) = String::from_utf8(chunk.to_vec())
                .ok()
                .map(|entry| sanitize_history_entry(&sanitize_input_text(&entry)))
                .filter(|entry| !entry.trim().is_empty())
            else {
                continue;
            };
            if seen.insert(command.clone()) {
                loaded.push(command);
            }
        }
        if loaded.len() > HISTORY_LIMIT {
            let keep_from = loaded.len() - HISTORY_LIMIT;
            loaded.drain(0..keep_from);
        }
        self.favorite_commands = loaded;
    }

    /// Frame-ready snapshot of the active completion flash. Composer
    /// pulls this each render to decide whether to paint the success
    /// highlight or apply the no-match shake. Returns `None` once the
    /// flash has elapsed past its duration.
    pub fn flash_state(&self) -> Option<CompletionFlashState> {
        match self.completion_flash? {
            CompletionFlash::Success { started, range } => {
                let elapsed_ms = Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
                    * 1000.0;
                if elapsed_ms >= SUCCESS_FLASH_MS {
                    return None;
                }
                let intensity = 1.0 - (elapsed_ms / SUCCESS_FLASH_MS).clamp(0.0, 1.0);
                Some(CompletionFlashState::Success { range, intensity })
            }
            CompletionFlash::NoMatch { started } => {
                let elapsed_ms = Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
                    * 1000.0;
                if elapsed_ms >= NO_MATCH_FLASH_MS {
                    return None;
                }
                let t = (elapsed_ms / NO_MATCH_FLASH_MS).clamp(0.0, 1.0);
                let intensity = 1.0 - t;
                // 4 oscillations across the duration; amplitude decays
                // with intensity so the wobble tapers naturally.
                let phase =
                    (elapsed_ms / NO_MATCH_FLASH_MS) * std::f32::consts::TAU * 4.0;
                let shake_offset_logical = NO_MATCH_SHAKE_AMP * intensity * phase.sin();
                Some(CompletionFlashState::NoMatch {
                    shake_offset_logical,
                    intensity,
                })
            }
        }
    }

    /// Raw editable text (no chevron prefix). Used by the composer
    /// overlay to draw the user's command.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte offset of the caret inside `text()`. Composer uses this to
    /// position the caret rect and split before/after slices for
    /// independent measure passes.
    pub fn cursor_byte(&self) -> usize {
        self.cursor
    }

    /// Borrowed history-suggestion suffix (Fish-style ghost). `None` when
    /// the cursor isn't at end-of-text or no history entry shares the
    /// current prefix. Cheap — recomputed on demand each frame.
    pub fn suggestion_after_cursor(&self) -> Option<&str> {
        self.suggestion_suffix()
    }

    /// Active completion menu items (already prefixed with `>` for the
    /// selected entry). Empty vector means no completion popup.
    pub fn completion_items(&self) -> &[String] {
        &self.completion_items
    }

    pub fn completion_detail(&self) -> Option<&str> {
        self.completion_detail.as_deref()
    }

    pub fn control_notice(&self) -> Option<&'static str> {
        let (notice, started) = self.control_notice?;
        if Instant::now()
            .saturating_duration_since(started)
            .as_secs_f32()
            > 1.2
        {
            return None;
        }
        Some(match notice {
            ControlNotice::Interrupt => "^C",
        })
    }

    pub fn show_interrupt_notice(&mut self) {
        self.control_notice = Some((ControlNotice::Interrupt, Instant::now()));
    }

    pub fn completion_menu_active(&self) -> bool {
        !self.completion_items.is_empty()
    }

    pub fn dismiss_completion_menu(&mut self) -> bool {
        if self.completion_items.is_empty() {
            return false;
        }
        self.completion_items.clear();
        self.completion_detail = None;
        self.completion_state = None;
        self.desired_visual_column = None;
        true
    }

    /// Snapshot of the most recent command blocks (oldest → newest)
    /// for the renderer's Warp-style block overlays. Reserved for the
    /// upcoming per-block sugarloaf overlay pass that will paint
    /// rounded card backings + status pills outside the cell grid.
    #[allow(dead_code)]
    pub fn command_block_snapshots(&self) -> Vec<CommandBlockSnapshot> {
        let favorites = self
            .favorite_commands
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        self.command_blocks
            .iter()
            .map(|block| CommandBlockSnapshot {
                command: block.command.clone(),
                cwd: block.cwd.clone(),
                status: match block.status {
                    TerminalCommandBlockStatus::Running => BlockStatusKind::Running,
                    TerminalCommandBlockStatus::Finished { exit_code: None } => {
                        BlockStatusKind::Ok
                    }
                    TerminalCommandBlockStatus::Finished { exit_code: Some(0) } => {
                        BlockStatusKind::Ok
                    }
                    TerminalCommandBlockStatus::Finished {
                        exit_code: Some(code),
                    } => BlockStatusKind::Error(code),
                },
                favorite: favorites.contains(&block.command),
                output_start_row: block.output_start_row,
                duration_ms: duration_ms(block),
            })
            .collect()
    }

    pub fn running_command_prefers_hidden_cursor(&self) -> bool {
        self.command_blocks
            .last()
            .filter(|block| matches!(block.status, TerminalCommandBlockStatus::Running))
            .is_some_and(|block| command_prefers_hidden_cursor(&block.command))
    }

    /// Burst-animation phase for the chevron lock-in effect, in
    /// milliseconds since submit. `None` once the burst has elapsed.
    pub fn prompt_burst_elapsed_ms(&self) -> Option<f32> {
        self.prompt_burst_elapsed_ms_internal()
    }

    /// Hard reset — drop every block snapshot regardless of position
    /// or status. Wired to the user submitting `clear` so the next
    /// command lands on a truly empty viewport with no leftover block
    /// chrome from prior output.
    pub fn clear_all_blocks(&mut self) {
        self.command_blocks.clear();
    }

    pub fn clear_previous_blocks_for_active_command(&mut self) {
        if self.command_blocks.last().is_some_and(|block| {
            matches!(block.status, TerminalCommandBlockStatus::Running)
        }) {
            let last = self.command_blocks.pop().unwrap();
            self.command_blocks.clear();
            self.command_blocks.push(last);
        } else {
            self.command_blocks.clear();
        }
    }

    pub fn sync_shell_state(&mut self, state: ShellPromptState) -> bool {
        // Latch the first prompt: once shell integration reports an
        // editable prompt we trust `awaiting_command` from here on, but
        // the boot window before it must still hand the empty command
        // line to the composer (see `should_capture_input`). Set this
        // before the early returns below so it latches even on the very
        // first prompt of a brand-new pane (no command block yet).
        if state.awaiting_command || state.running_command {
            self.ever_awaited_command = true;
        }
        if self.passthrough_session_active && state.awaiting_command {
            self.passthrough_session_active = false;
        }
        let Some(block) = self.command_blocks.last_mut() else {
            return false;
        };
        if !matches!(block.status, TerminalCommandBlockStatus::Running) {
            return false;
        }
        if state.running_command {
            block.saw_command_start = true;
        } else if state.awaiting_command
            && (block.saw_command_start
                || Instant::now().saturating_duration_since(block.submitted_at)
                    > Duration::from_millis(150))
        {
            let clear_completed = is_clear_command(&block.command);
            block.status = TerminalCommandBlockStatus::Finished {
                exit_code: state.last_exit_code,
            };
            block.finished_at = Some(Instant::now());
            if clear_completed {
                self.command_blocks.clear();
                return true;
            }
        }
        false
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let text = sanitize_input_text(text);
        if text.is_empty() {
            return;
        }
        self.reset_transient_edit_state();
        self.text.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub fn insert_paste(&mut self, text: &str) {
        let mut text = sanitize_input_text(text)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        while text.ends_with('\n') {
            text.pop();
        }
        self.insert_str(&text);
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.reset_transient_edit_state();
    }

    pub fn submit_passthrough(&mut self) -> String {
        let command = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.reset_transient_edit_state();
        self.trigger_prompt_burst();
        if !command.trim().is_empty() {
            self.push_history(command.clone());
        }
        command
    }

    #[cfg(test)]
    pub fn submit(&mut self) -> String {
        self.submit_with_cwd(None)
    }

    #[cfg(test)]
    pub fn submit_with_cwd(&mut self, cwd: Option<&Path>) -> String {
        self.submit_with_context(cwd, None)
    }

    pub fn submit_with_context(
        &mut self,
        cwd: Option<&Path>,
        output_start_row: Option<usize>,
    ) -> String {
        let command =
            sanitize_history_entry(&sanitize_input_text(&std::mem::take(&mut self.text)));
        self.cursor = 0;
        self.reset_transient_edit_state();
        self.trigger_prompt_burst();
        if !command.trim().is_empty() {
            if !is_clear_command(&command) {
                self.command_blocks.retain(|block| {
                    !(matches!(block.status, TerminalCommandBlockStatus::Finished { .. })
                        && is_clear_command(&block.command))
                });
            }
            self.push_history(command.clone());
            self.push_command_block(
                command.clone(),
                cwd.map(display_path),
                output_start_row,
            );
        }
        command
    }

    pub(crate) fn trigger_prompt_burst(&mut self) {
        self.prompt_burst_started = Some(Instant::now());
    }

    pub(crate) fn prompt_burst_elapsed_ms_internal(&self) -> Option<f32> {
        let started = self.prompt_burst_started?;
        let elapsed = Instant::now().saturating_duration_since(started);
        (elapsed < Duration::from_millis(PROMPT_BURST_MS as u64))
            .then(|| elapsed.as_secs_f32() * 1000.0)
    }

    pub(crate) fn reset_transient_edit_state(&mut self) {
        self.desired_visual_column = None;
        self.history_cursor = None;
        self.history_draft.clear();
        self.history_prefix = None;
        self.completion_items.clear();
        self.completion_detail = None;
        self.completion_state = None;
    }

    pub(crate) fn push_command_block(
        &mut self,
        command: String,
        cwd: Option<String>,
        output_start_row: Option<usize>,
    ) {
        if std::env::var_os("NEOISM_BLOCK_LOG").is_some()
            || std::env::var_os("NEOISM_SCROLL_LOG").is_some()
        {
            eprintln!(
                "[neoism block-submit] next_idx={} command={:?} output_start_row={:?} cwd={:?}",
                self.command_blocks.len(),
                command,
                output_start_row,
                cwd,
            );
        }
        self.command_blocks.push(TerminalCommandBlock {
            command,
            status: TerminalCommandBlockStatus::Running,
            saw_command_start: false,
            submitted_at: Instant::now(),
            finished_at: None,
            cwd,
            output_start_row,
        });
    }

    pub(crate) fn push_history(&mut self, command: String) {
        let command = sanitize_history_entry(&command);
        if command.trim().is_empty() {
            return;
        }
        if self.history.last() == Some(&command) {
            return;
        }
        self.history.push(command.clone());
        if self.history.len() > HISTORY_LIMIT {
            self.history.remove(0);
        }
        self.append_history_to_disk(&command);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn append_history_to_disk(&self, command: &str) {
        let Some(history) = self.persistent_history.as_ref() else {
            return;
        };
        if matches!(history, PersistentHistory::Zsh(_)) {
            // zsh owns HISTFILE writes. The composer keeps an in-memory
            // copy for immediate suggestions, but duplicating entries here
            // would make native zsh history noisy.
            return;
        }
        let path = history.path();
        let Some(parent) = path.parent() else {
            return;
        };
        let _ = std::fs::create_dir_all(parent);
        let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
            return;
        };
        tracing::trace!(
            target: "neoism::terminal_history",
            path = %path.display(),
            command_len = command.len(),
            "appending terminal command history"
        );
        let _ = file.write_all(command.as_bytes());
        let _ = file.write_all(&[0]);
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn append_history_to_disk(&self, _command: &str) {}

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn write_favorites_to_disk(&self) {
        let Some(path) = self.persistent_favorites.as_ref() else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        let _ = std::fs::create_dir_all(parent);
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
        else {
            return;
        };
        for command in &self.favorite_commands {
            let _ = file.write_all(command.as_bytes());
            let _ = file.write_all(&[0]);
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn write_favorites_to_disk(&self) {}

    pub(crate) fn suggestion_suffix(&self) -> Option<&str> {
        if self.text.is_empty() || self.cursor != self.text.len() {
            return None;
        }
        if !history_suggestion_allowed(&self.text) {
            return None;
        }
        self.history.iter().rev().find_map(|command| {
            command
                .strip_prefix(&self.text)
                .filter(|suffix| !suffix.is_empty())
        })
    }

    pub fn accept_suggestion(&mut self) -> bool {
        let Some(suffix) = self.suggestion_suffix().map(ToOwned::to_owned) else {
            return false;
        };
        self.reset_transient_edit_state();
        self.text.push_str(&suffix);
        self.cursor = self.text.len();
        true
    }

    pub fn complete_or_accept(&mut self, cwd: Option<&Path>) -> bool {
        let prev_text = self.text.clone();
        let prev_cursor = self.cursor;
        let result = self.complete_or_accept_inner(cwd);
        self.update_completion_flash(&prev_text, prev_cursor, result);
        result
    }

    pub(crate) fn update_completion_flash(
        &mut self,
        prev_text: &str,
        prev_cursor: usize,
        result: bool,
    ) {
        if result {
            // Find the byte interval that was newly inserted —
            // longest-common-prefix to longest-common-suffix gives
            // the changed window. The composer paints a fading accent
            // tint over that range so the user sees what completed.
            let common_prefix = prev_text
                .as_bytes()
                .iter()
                .zip(self.text.as_bytes().iter())
                .take_while(|(a, b)| a == b)
                .count();
            let new_end = self.text.len();
            let range_end = new_end.max(common_prefix);
            let range_start = common_prefix.min(self.cursor);
            // Skip the flash if nothing visibly changed (e.g. the
            // completion was a no-op that only reset transient state).
            if range_end > range_start && self.text != prev_text {
                self.completion_flash = Some(CompletionFlash::Success {
                    started: Instant::now(),
                    range: (range_start, range_end),
                });
            }
        } else if !self.text.is_empty() {
            // Tab fired but produced nothing — shake the buffer red.
            // Skipped on empty input so a stray Tab on an empty line
            // doesn't flash.
            self.completion_flash = Some(CompletionFlash::NoMatch {
                started: Instant::now(),
            });
        }
        let _ = prev_cursor;
    }

    pub(crate) fn complete_or_accept_inner(&mut self, cwd: Option<&Path>) -> bool {
        self.desired_visual_column = None;
        self.history_cursor = None;
        self.history_draft.clear();

        if self.advance_completion_cycle() {
            return true;
        }
        self.completion_state = None;
        self.completion_items.clear();

        let (start, end) = self.current_word_bounds();
        if start != end && self.cursor != end {
            return false;
        }
        let token = self.text[start..self.cursor].to_string();
        let token_len = token.len();
        let command_position = self.text[..start].trim().is_empty();
        let command_name = self.text.split_whitespace().next().unwrap_or_default();
        let candidates = completion_candidates(
            &token,
            &self.text[..start],
            command_position,
            command_name.eq_ignore_ascii_case("cd"),
            cwd,
        );
        if candidates.is_empty() {
            return history_suggestion_allowed(&self.text) && self.accept_suggestion();
        }

        let replacements = candidates
            .iter()
            .map(|candidate| candidate.replacement.as_str())
            .collect::<Vec<_>>();
        let common = common_prefix_case_insensitive(&replacements);
        if !common.is_empty() && (common.len() > token_len || !common.starts_with(&token))
        {
            self.text.replace_range(start..self.cursor, &common);
            self.cursor = start + common.len();
            if candidates.len() > 1 {
                self.set_completion_cycle(start, self.cursor, candidates, None);
            } else {
                self.set_completion_display(&candidates, Some(0));
            }
            return true;
        }

        if candidates.len() == 1 {
            let replacement = &candidates[0].replacement;
            self.set_completion_display(&candidates, Some(0));
            if replacement.len() > token_len || !replacement.starts_with(&token) {
                self.text.replace_range(start..self.cursor, replacement);
                self.cursor = start + replacement.len();
                return true;
            }
            if !replacement.ends_with('/') && self.cursor == self.text.len() {
                self.text.insert(self.cursor, ' ');
                self.cursor += 1;
                return true;
            }
        }

        self.set_completion_cycle(start, self.cursor, candidates, None);
        self.advance_completion_cycle()
    }

    pub(crate) fn set_completion_cycle(
        &mut self,
        start: usize,
        end: usize,
        candidates: Vec<CompletionCandidate>,
        selected: Option<usize>,
    ) {
        self.set_completion_display(&candidates, selected);
        self.completion_state = Some(CompletionCycle {
            start,
            end,
            candidates,
            selected: selected.unwrap_or(usize::MAX),
        });
    }

    pub(crate) fn advance_completion_cycle(&mut self) -> bool {
        self.move_completion_selection(1)
    }

    pub fn completion_next(&mut self) -> bool {
        self.move_completion_selection(1)
    }

    pub fn completion_previous(&mut self) -> bool {
        self.move_completion_selection(-1)
    }

    pub(crate) fn move_completion_selection(&mut self, delta: i32) -> bool {
        let Some(state) = self.completion_state.as_mut() else {
            return false;
        };
        if state.candidates.is_empty() || state.start > self.text.len() {
            self.completion_state = None;
            self.completion_items.clear();
            return false;
        }

        let current_end = self.cursor.min(self.text.len());
        if state.start > current_end {
            self.completion_state = None;
            self.completion_items.clear();
            return false;
        }

        let len = state.candidates.len();
        let current = (state.selected != usize::MAX).then_some(state.selected);
        let next = match (current, delta.signum()) {
            (None, -1) => len - 1,
            (None, _) => 0,
            (Some(ix), -1) => ix.checked_sub(1).unwrap_or(len - 1),
            (Some(ix), _) => (ix + 1) % len,
        };
        let replacement = state.candidates[next].replacement.clone();
        self.text
            .replace_range(state.start..current_end, &replacement);
        state.end = state.start + replacement.len();
        state.selected = next;
        self.cursor = state.end;
        self.desired_visual_column = None;
        self.completion_items = completion_labels(&state.candidates, Some(next));
        self.completion_detail = completion_detail(&state.candidates, Some(next));
        true
    }

    pub(crate) fn set_completion_display(
        &mut self,
        candidates: &[CompletionCandidate],
        selected: Option<usize>,
    ) {
        self.completion_items = completion_labels(candidates, selected);
        self.completion_detail = completion_detail(candidates, selected);
    }

    pub fn history_previous(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        self.completion_items.clear();
        let prefix = self.history_prefix.clone().unwrap_or_else(|| {
            self.history_draft = self.text.clone();
            let prefix = self.text[..self.cursor].trim_start().to_string();
            self.history_prefix = Some(prefix.clone());
            prefix
        });
        let start = self
            .history_cursor
            .map(|idx| idx.saturating_sub(1))
            .unwrap_or_else(|| self.history.len().saturating_sub(1));
        let Some(next) = self.history_match_previous(start, &prefix) else {
            return false;
        };
        self.history_cursor = Some(next);
        self.text = sanitize_history_entry(&self.history[next]);
        self.cursor = self.text.len();
        self.desired_visual_column = None;
        true
    }

    pub fn history_next(&mut self) -> bool {
        let Some(idx) = self.history_cursor else {
            return false;
        };
        self.completion_items.clear();
        let prefix = self.history_prefix.clone().unwrap_or_default();
        if let Some(next) = self.history_match_next(idx + 1, &prefix) {
            self.history_cursor = Some(next);
            self.text = sanitize_history_entry(&self.history[next]);
        } else {
            self.history_cursor = None;
            self.text = std::mem::take(&mut self.history_draft);
        }
        self.cursor = self.text.len();
        self.desired_visual_column = None;
        true
    }

    pub fn open_history_picker(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let query = self.text[..self.cursor].trim_start().to_string();
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        for command in self.history.iter().rev() {
            let command = sanitize_history_entry(command);
            if command.is_empty() || !seen.insert(command.clone()) {
                continue;
            }
            if history_prefix_match(&command, &query)
                || history_fuzzy_match(&command, &query)
            {
                candidates.push(CompletionCandidate {
                    replacement: command.clone(),
                    label: command,
                    kind: CompletionKind::History,
                    detail: Some(if query.is_empty() {
                        "Recent command".to_string()
                    } else {
                        format!("History match for `{query}`")
                    }),
                    sort_group: 0,
                });
            }
            if candidates.len() >= 64 {
                break;
            }
        }
        if candidates.is_empty() {
            return false;
        }
        let replacement = candidates[0].replacement.clone();
        self.text = replacement;
        self.cursor = self.text.len();
        self.desired_visual_column = None;
        self.history_cursor = None;
        self.history_draft.clear();
        self.history_prefix = None;
        self.set_completion_cycle(0, self.cursor, candidates, Some(0));
        true
    }

    pub fn open_favorite_picker(&mut self) -> bool {
        if self.favorite_commands.is_empty() {
            return false;
        }
        let query = self.text[..self.cursor].trim_start().to_string();
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::new();
        for command in self.favorite_commands.iter().rev() {
            let command = sanitize_history_entry(command);
            if command.is_empty() || !seen.insert(command.clone()) {
                continue;
            }
            if history_prefix_match(&command, &query)
                || history_fuzzy_match(&command, &query)
            {
                candidates.push(CompletionCandidate {
                    replacement: command.clone(),
                    label: command,
                    kind: CompletionKind::Favorite,
                    detail: Some(if query.is_empty() {
                        "Favorite command".to_string()
                    } else {
                        format!("Favorite match for `{query}`")
                    }),
                    sort_group: 0,
                });
            }
            if candidates.len() >= 64 {
                break;
            }
        }
        if candidates.is_empty() {
            return false;
        }
        let replacement = candidates[0].replacement.clone();
        self.text = replacement;
        self.cursor = self.text.len();
        self.desired_visual_column = None;
        self.history_cursor = None;
        self.history_draft.clear();
        self.history_prefix = None;
        self.set_completion_cycle(0, self.cursor, candidates, Some(0));
        true
    }

    pub fn toggle_favorite_command(&mut self, command: &str) -> Option<bool> {
        let command = sanitize_history_entry(&sanitize_input_text(command));
        if command.trim().is_empty() {
            return None;
        }
        if let Some(pos) = self
            .favorite_commands
            .iter()
            .position(|entry| entry == &command)
        {
            self.favorite_commands.remove(pos);
            self.write_favorites_to_disk();
            return Some(false);
        }
        self.favorite_commands.push(command);
        if self.favorite_commands.len() > HISTORY_LIMIT {
            self.favorite_commands.remove(0);
        }
        self.write_favorites_to_disk();
        Some(true)
    }

    pub(crate) fn history_match_previous(
        &self,
        start: usize,
        prefix: &str,
    ) -> Option<usize> {
        (0..=start)
            .rev()
            .find(|idx| history_prefix_match(&self.history[*idx], prefix))
            .or_else(|| {
                (0..=start)
                    .rev()
                    .find(|idx| history_fuzzy_match(&self.history[*idx], prefix))
            })
    }

    pub(crate) fn history_match_next(&self, start: usize, prefix: &str) -> Option<usize> {
        (start..self.history.len())
            .find(|idx| history_prefix_match(&self.history[*idx], prefix))
            .or_else(|| {
                (start..self.history.len())
                    .find(|idx| history_fuzzy_match(&self.history[*idx], prefix))
            })
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.reset_transient_edit_state();
        let prev = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        self.reset_transient_edit_state();
        let next = self.text[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(idx, _)| self.cursor + idx)
            .unwrap_or(self.text.len());
        self.text.drain(self.cursor..next);
    }

    pub fn delete_to_end(&mut self) {
        self.reset_transient_edit_state();
        self.text.truncate(self.cursor);
    }

    pub fn delete_previous_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.reset_transient_edit_state();
        let mut word_start = 0;
        let mut seen_word = false;
        for (idx, ch) in self.text[..self.cursor].char_indices().rev() {
            if ch.is_whitespace() {
                if seen_word {
                    word_start = idx + ch.len_utf8();
                    break;
                }
            } else {
                seen_word = true;
                word_start = idx;
            }
        }
        self.text.drain(word_start..self.cursor);
        self.cursor = word_start;
    }

    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.desired_visual_column = None;
        self.completion_items.clear();
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(idx, _)| idx)
            .unwrap_or(0);
    }

    pub fn move_right(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        self.desired_visual_column = None;
        self.completion_items.clear();
        self.cursor = self.text[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(idx, _)| self.cursor + idx)
            .unwrap_or(self.text.len());
    }

    pub fn is_multiline(&self) -> bool {
        self.text.contains('\n')
    }

    pub fn move_visual_up(&mut self) -> bool {
        let (line_start, _) = self.current_line_bounds();
        if line_start == 0 {
            return false;
        }
        let previous_line_end = line_start - 1;
        let previous_line_start = self.text[..previous_line_end]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let column = self
            .desired_visual_column
            .unwrap_or_else(|| self.text[line_start..self.cursor].chars().count());
        self.completion_items.clear();
        self.cursor = byte_at_char_column(
            &self.text,
            previous_line_start,
            previous_line_end,
            column,
        );
        self.desired_visual_column = Some(column);
        true
    }

    pub fn move_visual_up_in_ranges(&mut self, ranges: &[(usize, usize)]) -> bool {
        self.move_in_visual_ranges(ranges, -1)
    }

    pub fn move_visual_down(&mut self) -> bool {
        let (line_start, line_end) = self.current_line_bounds();
        if line_end >= self.text.len() {
            return false;
        }
        let next_line_start = line_end + 1;
        let next_line_end = self.text[next_line_start..]
            .find('\n')
            .map(|idx| next_line_start + idx)
            .unwrap_or(self.text.len());
        let column = self
            .desired_visual_column
            .unwrap_or_else(|| self.text[line_start..self.cursor].chars().count());
        self.completion_items.clear();
        self.cursor =
            byte_at_char_column(&self.text, next_line_start, next_line_end, column);
        self.desired_visual_column = Some(column);
        true
    }

    pub fn move_visual_down_in_ranges(&mut self, ranges: &[(usize, usize)]) -> bool {
        self.move_in_visual_ranges(ranges, 1)
    }

    pub fn move_home(&mut self) {
        self.desired_visual_column = None;
        self.completion_items.clear();
        self.cursor = if self.is_multiline() {
            self.current_line_bounds().0
        } else {
            0
        };
    }

    pub fn move_end(&mut self) {
        self.desired_visual_column = None;
        self.completion_items.clear();
        self.cursor = if self.is_multiline() {
            self.current_line_bounds().1
        } else {
            self.text.len()
        };
    }

    pub(crate) fn move_in_visual_ranges(
        &mut self,
        ranges: &[(usize, usize)],
        delta: isize,
    ) -> bool {
        if ranges.len() <= 1 {
            return false;
        }
        let current_idx = self.current_visual_range_index(ranges);
        let Some(target_idx) = current_idx.checked_add_signed(delta) else {
            return false;
        };
        if target_idx >= ranges.len() {
            return false;
        }

        let (line_start, _) = ranges[current_idx];
        let (target_start, target_end) = ranges[target_idx];
        let column = self.desired_visual_column.unwrap_or_else(|| {
            self.text[line_start.min(self.cursor)..self.cursor]
                .chars()
                .count()
        });
        self.completion_items.clear();
        self.cursor = byte_at_char_column(&self.text, target_start, target_end, column);
        self.desired_visual_column = Some(column);
        true
    }

    pub(crate) fn current_visual_range_index(&self, ranges: &[(usize, usize)]) -> usize {
        // Inclusive end, first match: a cursor sitting exactly on a
        // soft-wrap boundary (end of row N == start of row N+1)
        // belongs to row N. The old exclusive-end test bounced it back
        // to row N+1, so Up from a long row onto a shorter one clamped
        // to the boundary and every further Up re-resolved the SAME
        // row — the "stuck bouncing" loop. It also let a cursor at a
        // hard line's end (== end, < next start) match nothing and
        // fall through to the LAST row. Must stay in sync with the
        // composer renderer's `line_for_byte`.
        for (idx, &(start, end)) in ranges.iter().enumerate() {
            if self.cursor >= start && self.cursor <= end {
                return idx;
            }
        }
        ranges.len().saturating_sub(1)
    }

    pub(crate) fn current_line_bounds(&self) -> (usize, usize) {
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let end = self.text[self.cursor..]
            .find('\n')
            .map(|idx| self.cursor + idx)
            .unwrap_or(self.text.len());
        (start, end)
    }

    pub(crate) fn current_word_bounds(&self) -> (usize, usize) {
        let start = self.text[..self.cursor]
            .char_indices()
            .rev()
            .find_map(|(idx, ch)| ch.is_whitespace().then_some(idx + ch.len_utf8()))
            .unwrap_or(0);
        let end = self.text[self.cursor..]
            .char_indices()
            .find_map(|(idx, ch)| ch.is_whitespace().then_some(self.cursor + idx))
            .unwrap_or(self.text.len());
        (start, end)
    }
}
