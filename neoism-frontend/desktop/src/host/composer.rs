use super::path_exec::{build_path_executables, display_cwd, SHELL_BUILTINS};
use super::*;
use crate::terminal::blocks::input::TerminalInputBufferHostExt;

pub(super) fn composer_style(
    color: [u8; 4],
    bold: bool,
    underline: bool,
) -> command_composer::InputTextStyle {
    command_composer::InputTextStyle {
        color,
        bold,
        underline,
    }
}

pub(super) fn composer_rgb(hex: u32) -> [u8; 4] {
    [
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
        255,
    ]
}

/// Classify the buffered command string for the composer's
/// zsh-syntax-style coloring. The palette mirrors the user's zsh config:
/// command/default blue bold, git green bold, package tools yellow bold,
/// nix/builtins purple bold, dangerous/unknown red, paths underlined.
///
/// Rules (matches what zsh-syntax-highlighting does for the common
/// case):
/// - Empty input → both segments use plain foreground.
/// - First token uses command/pattern colors from zsh-syntax-highlighting.
/// - Unknown command → red.
/// - For `cd <path>`, the path argument turns red when the target
///   doesn't exist (relative to `cwd`).
pub(super) fn classify_input(
    text: &str,
    cwd: Option<&std::path::Path>,
    path_executables: &rustc_hash::FxHashSet<String>,
    theme: &IdeTheme,
) -> command_composer::InputClassification {
    let fg = theme.u8(theme.fg);
    if text.trim().is_empty() {
        return command_composer::InputClassification::neutral(fg);
    }

    let alias_green = composer_rgb(0x9fe8c3);
    let arg_gray = composer_rgb(0xb5bcc9);
    let purple = composer_rgb(0xc2a2e3);
    let command_blue = composer_rgb(0x99aee5);
    let path_blue = composer_rgb(0xb5c3ea);
    let yellow = composer_rgb(0xfbdf90);
    let red = composer_rgb(0xef8891);

    // Split on first whitespace.
    let cmd_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let cmd = &text[..cmd_end];
    let rest = text[cmd_end..].trim_start();

    let cmd_resolves = if cmd.is_empty() {
        false
    } else if SHELL_BUILTINS.contains(&cmd) {
        true
    } else if cmd.starts_with('/') || cmd.starts_with("./") || cmd.starts_with("../") {
        let candidate = if cmd.starts_with('/') {
            std::path::PathBuf::from(cmd)
        } else {
            cwd.map(|c| c.join(cmd))
                .unwrap_or_else(|| std::path::PathBuf::from(cmd))
        };
        candidate.is_file()
    } else {
        path_executables.contains(cmd)
    };

    let command = if matches!(cmd, "rm" | "sudo") {
        composer_style(red, true, false)
    } else if cmd.starts_with("cargo")
        || cmd.starts_with("npm")
        || cmd.starts_with("yarn")
    {
        composer_style(yellow, true, false)
    } else if cmd.starts_with("git") {
        composer_style(alias_green, true, false)
    } else if cmd.starts_with("docker")
        || cmd.starts_with("k9s")
        || cmd.starts_with("kubectl")
    {
        composer_style(command_blue, true, false)
    } else if cmd.starts_with("nix") || cmd.starts_with("nixos-rebuild") {
        composer_style(purple, true, false)
    } else if SHELL_BUILTINS.contains(&cmd) {
        composer_style(purple, true, false)
    } else if cmd_resolves {
        composer_style(command_blue, true, false)
    } else {
        composer_style(red, false, false)
    };

    // Args coloring — for `cd`, validate the dir; for other commands
    // keep the args muted/foreground so they read as quieter than the
    // colored command word.
    let arg = if cmd == "cd" && !rest.is_empty() {
        let target = if rest.starts_with('/') {
            std::path::PathBuf::from(rest)
        } else if rest.starts_with("~/") {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(&rest[2..]))
                .unwrap_or_else(|| std::path::PathBuf::from(rest))
        } else {
            cwd.map(|c| c.join(rest))
                .unwrap_or_else(|| std::path::PathBuf::from(rest))
        };
        if target.is_dir() {
            composer_style(path_blue, false, true)
        } else {
            composer_style(red, false, false)
        }
    } else {
        composer_style(arg_gray, false, false)
    };

    command_composer::InputClassification {
        command,
        arg,
        string: composer_style(yellow, false, false),
        path: composer_style(path_blue, false, true),
        glob: composer_style(alias_green, false, false),
        redirection: composer_style(purple, false, false),
    }
}

/// How many cell rows the Warp-style command composer reserves
/// at the bottom of `ctx`'s pane right now. Used by the splash
/// inject + render so the wordmark / menu never extends into
/// rows the composer is going to overlap. Returns 0 when the
/// composer footer is hidden (alt-screen TUI, passthrough, no
/// active prompt).
pub(super) fn splash_composer_reserved_rows<T: neoism_backend::event::EventListener>(
    ctx: &crate::context::Context<T>,
    composer: &command_composer::CommandComposer,
    scale_factor: f32,
) -> usize {
    let Some(terminal) = ctx.terminal.try_lock_unfair() else {
        return 0;
    };
    let alt = terminal
        .mode()
        .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN);
    let prompt = terminal.shell_prompt_state();
    drop(terminal);

    // Match `composer_footer_active` so the splash doesn't leave a
    // reserved gap at the bottom while a CLI / TUI is taking the
    // pane.
    if alt || prompt.running_command || ctx.terminal_input.passthrough_session_active() {
        return 0;
    }
    // Match `composer_footer_active`: reserve the composer gap during
    // the fresh-terminal boot window too, so the first command's
    // composer doesn't overlap the splash before the first prompt.
    let block_input_active = ctx.terminal_input.editing_window_open(prompt);
    if !ctx.terminal_input.has_visible_footer(block_input_active) {
        return 0;
    }
    let cell_h = ctx.dimension.dimension.height.round().max(1.0);
    let cell_w = ctx.dimension.dimension.width.round().max(1.0);
    let cell_h_logical = (cell_h / scale_factor).max(1.0);
    let cell_w_logical = (cell_w / scale_factor).max(1.0);
    composer.terminal_reserved_rows_for_input(
        cell_h_logical,
        ctx.dimension.columns as f32 * cell_w_logical,
        cell_w_logical,
        ctx.dimension.lines,
        ctx.terminal_input.text(),
    )
}

impl Renderer {
    /// Paint parked composers for inactive terminal splits. The active
    /// composer is stateful because it owns completion/caret animation; these
    /// previews are intentionally throwaway so inactive shells keep their
    /// prompt surface without stealing the active composer's hit testing.
    pub(super) fn render_inactive_command_composers(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        context_manager: &ContextManager<EventProxy>,
        scale_factor: f32,
        logical_height: f32,
    ) {
        if self.path_executables.is_none() {
            self.path_executables = Some(build_path_executables());
        }
        let path_cache = self.path_executables.as_ref().expect("just populated");
        let theme = self.theme;
        let composer_scale = self.command_composer.scale();
        let status_h = self.status_line.scaled_height();
        let blink_interval = if self.config_has_blinking_enabled {
            self.config_blinking_interval
        } else {
            0
        };
        let animation_phase = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_secs() % 10_000) as f32 + d.subsec_nanos() as f32 / 1e9)
            .unwrap_or(0.0);

        let grid = context_manager.current_grid();
        let active_key = grid.current;
        let scaled_margin = grid.scaled_margin;
        let mut preview = command_composer::CommandComposer::new();
        preview.set_scale(composer_scale);
        preview.set_visible(true);

        for (node, item) in grid.contexts() {
            if *node == active_key || !grid.is_context_visible(*node) {
                continue;
            }
            let ctx = item.context();
            if ctx.has_non_terminal_surface() {
                continue;
            }

            let (alt_screen, prompt, running, cwd_path) = {
                let terminal = ctx.terminal.lock();
                let prompt = terminal.shell_prompt_state();
                (
                    terminal
                        .mode()
                        .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN),
                    prompt,
                    prompt.running_command,
                    terminal.current_directory.clone(),
                )
            };
            // Boot-window aware: match `composer_footer_active` so the
            // first command's composer paints before the first prompt.
            let awaiting = ctx.terminal_input.editing_window_open(prompt);
            let has_footer = ctx
                .terminal_input
                .has_visible_footer(awaiting && !alt_screen);
            if alt_screen
                || running
                || ctx.terminal_input.passthrough_session_active()
                || !has_footer
            {
                continue;
            }

            let rect = item.layout_rect;
            let pane_left = (rect[0] + scaled_margin.left) / scale_factor;
            let pane_top = (rect[1] + scaled_margin.top) / scale_factor;
            let pane_w = rect[2] / scale_factor;
            let pane_h = rect[3] / scale_factor;
            if pane_w <= 0.0 || pane_h <= 0.0 {
                continue;
            }

            let cell_w = (ctx.dimension.dimension.width.round() / scale_factor).max(1.0);
            let cell_h = (ctx.dimension.dimension.height.round() / scale_factor).max(1.0);
            let raw_chassis_h = preview.actual_chassis_height_for_input(
                cell_h,
                pane_w,
                cell_w,
                (pane_h / cell_h).floor().max(0.0) as usize,
                ctx.terminal_input.text(),
            );
            let top_pad = 14.0 * composer_scale;
            let chassis_h = (raw_chassis_h - top_pad).max(raw_chassis_h * 0.5);
            let composer_bottom_gap =
                command_composer::COMPOSER_BOTTOM_PAD * composer_scale;
            let max_bottom = (logical_height - status_h - composer_bottom_gap).max(0.0);
            let bottom = (pane_top + pane_h).min(max_bottom);
            let y_top = (bottom - chassis_h).max(pane_top);

            let cwd_label = cwd_path
                .as_ref()
                .map(|path| display_cwd(path))
                .unwrap_or_else(|| "~".to_string());
            let classification = classify_input(
                ctx.terminal_input.text(),
                cwd_path.as_deref(),
                path_cache,
                &theme,
            );
            preview.render(
                sugarloaf,
                pane_left,
                y_top,
                pane_w,
                chassis_h,
                &theme,
                &ctx.terminal_input,
                Some(cwd_label.as_str()),
                animation_phase,
                false,
                cell_w,
                cell_h,
                false,
                blink_interval,
                classification,
                ctx.terminal_shell_kind.into(),
            );
        }
    }

    /// Paint the Warp-style composer chassis for the active terminal
    /// pane (when one is in block-input mode). Pulls cwd + input state
    /// from the active context, geometry from the pane's `layout_rect`,
    /// and the prompt-burst animation phase from wall-clock time so the
    /// chevron rainbow stays in sync with the existing footer effect.
    pub(super) fn render_command_composer(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        context_manager: &mut ContextManager<EventProxy>,
        scale_factor: f32,
        logical_height: f32,
    ) {
        // Gate: active context must be a terminal (no editor/markdown
        // surface) and not running an alt-screen TUI.
        let (
            pane_left_logical,
            pane_top_logical,
            pane_width_logical,
            pane_height_logical,
            cwd_label,
            cwd_path,
            visible,
            focused,
            cell_w_logical,
            cell_h_logical,
        ) = {
            let current_grid = context_manager.current_grid();
            let scaled_margin = current_grid.scaled_margin;
            let Some(item) = current_grid.current_item() else {
                self.command_composer.set_visible(false);
                return;
            };
            let ctx = &item.val;
            if ctx.has_non_terminal_surface() {
                self.command_composer.set_visible(false);
                return;
            }
            let (alt_screen, prompt, running) = {
                let terminal = ctx.terminal.lock();
                let prompt = terminal.shell_prompt_state();
                (
                    terminal
                        .mode()
                        .contains(neoism_terminal_core::crosswords::Mode::ALT_SCREEN),
                    prompt,
                    prompt.running_command,
                )
            };
            // Visible whenever the user has block context (recent
            // command blocks or live awaiting prompt) — matches the
            // `has_visible_footer` predicate in the screen pipeline so
            // we don't pop in/out between frames. `editing_window_open`
            // also keeps the composer up during the fresh-terminal boot
            // window before the first prompt latches.
            let awaiting = ctx.terminal_input.editing_window_open(prompt);
            let has_footer = ctx
                .terminal_input
                .has_visible_footer(awaiting && !alt_screen);
            // Hide the composer (and its bg / reserved space) whenever
            // another program is driving the terminal: alt-screen TUI,
            // passthrough shells, or a foreground command in progress.
            // The CLI/TUI then runs against the bare terminal grid as
            // if the composer were never there.
            if alt_screen
                || running
                || ctx.terminal_input.passthrough_session_active()
                || !has_footer
            {
                self.command_composer.set_visible(false);
                return;
            }
            let rect = item.layout_rect;
            let pane_left = (rect[0] + scaled_margin.left) / scale_factor;
            let pane_top = (rect[1] + scaled_margin.top) / scale_factor;
            let pane_w = rect[2] / scale_factor;
            let pane_h = rect[3] / scale_factor;
            // ctx.dimension.dimension is in **physical** pixels; the
            // composer renders in **logical** pixels (sugarloaf scales
            // up internally). Convert here so the caret block matches
            // one terminal cell on screen instead of one physical-pixel
            // cell expressed in logical space (which would land twice
            // as tall on a hi-DPI display).
            let cell_w = (ctx.dimension.dimension.width.round() / scale_factor).max(1.0);
            let cell_h = (ctx.dimension.dimension.height.round() / scale_factor).max(1.0);
            let (cwd, cwd_path) = {
                let term = ctx.terminal.lock();
                let path = term.current_directory.clone();
                let label = path
                    .as_ref()
                    .map(|p| display_cwd(p))
                    .unwrap_or_else(|| "~".to_string());
                (label, path)
            };
            (
                pane_left, pane_top, pane_w, pane_h, cwd, cwd_path, true, true, cell_w,
                cell_h,
            )
        };

        if !visible {
            self.command_composer.set_visible(false);
            return;
        }
        self.command_composer.set_visible(true);

        // Composer hugs the pane's bottom edge but never crosses the
        // global status strip. Snap chassis_h to a whole-cell multiple
        // (`reserved_rows × cell_h`) so the chassis fills its row
        // reservation with no half-cell gap above — without this, the
        // round-up in `reserved_rows` leaves visible blanked cells
        // stacked between the last output line and the chassis top.
        let input_text = context_manager.current().terminal_input.text();
        let raw_chassis_h = self.command_composer.actual_chassis_height_for_input(
            cell_h_logical,
            pane_width_logical,
            cell_w_logical,
            (pane_height_logical / cell_h_logical.max(1.0))
                .floor()
                .max(0.0) as usize,
            input_text,
        );
        // Always-on top inset. The chassis's rounded lip
        // (`COMPOSER_TOP_OVERHANG`) used to clip the descenders of the
        // last PTY row sitting right above it. Earlier we tried to apply
        // this shrink only at the live tail (display_offset == 0), but
        // that made the chassis bob up/down every time scroll state
        // flipped. Apply it unconditionally so the rounded lip stays
        // inside the reserved composer band.
        let top_pad = 14.0 * self.command_composer.scale();
        let chassis_h = (raw_chassis_h - top_pad).max(raw_chassis_h * 0.5);
        let pane_bottom = pane_top_logical + pane_height_logical;
        // Keep the composer seated on the status strip. The status
        // line's higher draw order covers the join; an external bottom
        // gap shifts the composer's rounded top into the last reserved
        // terminal row and clips output at some zoom sizes.
        let composer_bottom_gap =
            command_composer::COMPOSER_BOTTOM_PAD * self.command_composer.scale();
        let max_bottom =
            (logical_height - self.status_line.scaled_height() - composer_bottom_gap)
                .max(0.0);
        let bottom = pane_bottom.min(max_bottom);
        let y_top = (bottom - chassis_h).max(pane_top_logical);

        // Animation phase = monotonic seconds-mod-10000, matching the
        // value the screen loop hands to the cell-grid prompt
        // animation; keeps the chevron rainbow phase-locked across the
        // two render paths during the cross-fade window.
        let animation_phase = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_secs() % 10_000) as f32 + d.subsec_nanos() as f32 / 1e9)
            .unwrap_or(0.0);

        // Read the input snapshot via a brief borrow; the grid's
        // `current_item` only lent us geometry/cwd so we drop that
        // borrow and reach back through `current()` here.
        let trail_will_paint = self.trail_cursor_enabled;
        let blink_interval = if self.config_has_blinking_enabled {
            self.config_blinking_interval
        } else {
            0
        };
        if self.path_executables.is_none() {
            self.path_executables = Some(build_path_executables());
        }
        let path_cache = self.path_executables.as_ref().expect("just populated");
        #[cfg(target_os = "linux")]
        {
            let current = context_manager.current_mut();
            if let Some(shell_kind) =
                crate::terminal::blocks::detect_foreground_shell(*current.main_fd)
            {
                if current.terminal_shell_kind != shell_kind {
                    current
                        .terminal_input
                        .enable_persistent_history_for_shell(shell_kind);
                }
                current.terminal_shell_kind = shell_kind;
            }
        }
        let current = context_manager.current();
        let classification = classify_input(
            current.terminal_input.text(),
            cwd_path.as_deref(),
            path_cache,
            &self.theme,
        );
        self.command_composer.render(
            sugarloaf,
            pane_left_logical,
            y_top,
            pane_width_logical,
            chassis_h,
            &self.theme,
            &current.terminal_input,
            Some(cwd_label.as_str()),
            animation_phase,
            focused,
            cell_w_logical,
            cell_h_logical,
            trail_will_paint,
            blink_interval,
            classification,
            current.terminal_shell_kind.into(),
        );
    }
}
