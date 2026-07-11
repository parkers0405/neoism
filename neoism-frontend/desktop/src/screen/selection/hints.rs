use super::*;

impl Screen<'_> {
    pub(crate) fn modifiers_match(&self, required_mods: &[String]) -> bool {
        hint_modifiers_match(required_mods, self.hint_modifier_state())
    }

    pub(crate) fn hint_modifier_state(&self) -> HintModifierState {
        let current_mods = self.modifiers.state();
        HintModifierState::new(
            current_mods.shift_key(),
            current_mods.control_key(),
            current_mods.alt_key(),
            current_mods.super_key(),
        )
    }

    pub(crate) fn hint_mouse_activations(
        &self,
    ) -> impl Iterator<Item = HintMouseActivation<'_>> {
        self.hints_config
            .iter()
            .map(|hint_config| HintMouseActivation {
                mouse_enabled: hint_config.mouse.enabled,
                hyperlinks: hint_config.hyperlinks,
                mouse_mods: &hint_config.mouse.mods,
            })
    }

    pub(crate) fn find_hint_at_point(
        &self,
        terminal: &neoism_terminal_core::crosswords::Crosswords,
        point: neoism_terminal_core::crosswords::pos::Pos,
        _modifiers: neoism_window::keyboard::ModifiersState,
    ) -> Option<crate::terminal::hints::HintMatch> {
        // Check each enabled hint configuration
        for hint_config in &self.hints_config {
            // Check if mouse highlighting is enabled for this hint
            if !hint_config.mouse.enabled {
                continue;
            }

            // Check if current modifiers match the required modifiers for this hint
            if !self.modifiers_match(&hint_config.mouse.mods) {
                continue;
            }

            // Check hyperlinks if enabled
            if hint_config.hyperlinks {
                if let Some(hyperlink_match) =
                    self.find_hyperlink_at_point(terminal, point)
                {
                    return Some(hyperlink_match);
                }
            }

            // Check regex patterns if specified
            if let Some(regex_pattern) = &hint_config.regex {
                if let Ok(regex) = onig::Regex::new(regex_pattern) {
                    if let Some(regex_match) = self.find_regex_match_at_point(
                        terminal,
                        point,
                        &regex,
                        hint_config.clone(),
                    ) {
                        return Some(regex_match);
                    }
                }
            }
        }

        None
    }

    pub(crate) fn find_hyperlink_at_point(
        &self,
        terminal: &neoism_terminal_core::crosswords::Crosswords,
        point: neoism_terminal_core::crosswords::pos::Pos,
    ) -> Option<crate::terminal::hints::HintMatch> {
        // Build a synthetic hint config so the rest of the hint
        // pipeline (highlighting, click action) treats this just like
        // a regex/url match.
        let hint_config = std::rc::Rc::new(neoism_backend::config::hints::Hint {
            regex: None,
            hyperlinks: true,
            post_processing: true,
            persist: false,
            action: neoism_backend::config::hints::HintAction::Command {
                command: neoism_backend::config::hints::HintCommand::Simple(
                    "xdg-open".to_string(),
                ),
            },
            mouse: neoism_backend::config::hints::HintMouse::default(),
            binding: None,
        });
        let span = hyperlink_span_at(terminal, point, hint_config.post_processing)?;

        Some(crate::terminal::hints::HintMatch {
            text: span.uri,
            start: span.start,
            end: span.end,
            hint: hint_config,
        })
    }

    pub(crate) fn find_regex_match_at_point(
        &self,
        terminal: &neoism_terminal_core::crosswords::Crosswords,
        point: neoism_terminal_core::crosswords::pos::Pos,
        regex: &onig::Regex,
        hint_config: std::rc::Rc<neoism_backend::config::hints::Hint>,
    ) -> Option<crate::terminal::hints::HintMatch> {
        let grid = &terminal.grid;

        // Check if the point is within grid bounds
        if point.row < grid.topmost_line()
            || point.row > grid.bottommost_line()
            || point.col.0 >= grid.columns()
        {
            return None;
        }

        // Extract text from the line
        let mut line_text = String::new();
        for col in 0..grid.columns() {
            let cell =
                &grid[point.row][neoism_terminal_core::crosswords::pos::Column(col)];
            line_text.push(cell.c());
        }
        let line_text = line_text.trim_end();

        // Find all matches in this line and check if point is within any of them.
        // Onig yields (byte_start, byte_end); we slice the source ourselves.
        for (start, end) in regex.find_iter(line_text) {
            let start_col = neoism_terminal_core::crosswords::pos::Column(start);
            let end_col =
                neoism_terminal_core::crosswords::pos::Column(end.saturating_sub(1));

            // Check if the point is within this match
            if point.col >= start_col && point.col <= end_col {
                let original_match_text = line_text[start..end].to_string();
                let mut match_text = original_match_text.clone();

                // Apply grid-based post-processing
                let (processed_start, processed_end) = if hint_config.post_processing {
                    self.hint_post_processing(
                        terminal,
                        start_col,
                        end_col,
                        neoism_terminal_core::crosswords::pos::Line(point.row.0),
                    )
                    .unwrap_or((start_col, end_col))
                } else {
                    (start_col, end_col)
                };

                // Extract the processed text
                if hint_config.post_processing {
                    let mut processed_text = String::new();
                    for col in processed_start.0..=processed_end.0 {
                        let cell = &grid[point.row]
                            [neoism_terminal_core::crosswords::pos::Column(col)];
                        processed_text.push(cell.c());
                    }
                    match_text = processed_text.trim_end().to_string();
                }

                return Some(crate::terminal::hints::HintMatch {
                    text: match_text,
                    start: neoism_terminal_core::crosswords::pos::Pos::new(
                        point.row,
                        processed_start,
                    ),
                    end: neoism_terminal_core::crosswords::pos::Pos::new(
                        point.row,
                        processed_end,
                    ),
                    hint: hint_config,
                });
            }
        }

        None
    }

    pub fn trigger_hyperlink(&self) -> bool {
        if !hyperlink_trigger_eligible(
            self.hint_mouse_activations(),
            self.hint_modifier_state(),
            self.context_manager.current().has_hyperlink_range(),
        ) {
            return false;
        }

        // Look up the cell under the mouse and dispatch open_hyperlink
        // if it carries an OSC 8 link.
        let terminal = self.context_manager.current().terminal.lock();
        let display_offset = terminal.display_offset();
        let Some(pos) = self.terminal_body_mouse_position(display_offset) else {
            return false;
        };
        let hyperlink_uri = hyperlink_span_at(&terminal, pos, true).map(|span| span.uri);
        drop(terminal);

        if let Some(uri) = hyperlink_uri {
            self.open_hyperlink_uri(&uri);
            return true;
        }

        false
    }

    pub fn trigger_hint(&mut self, clipboard: &mut Clipboard) -> bool {
        // Take the highlighted hint
        let hint_match = self
            .context_manager
            .current_mut()
            .renderable_content
            .highlighted_hint
            .take();

        if let Some(hint_match) = hint_match {
            self.execute_hint_action(&hint_match, clipboard);
            true
        } else {
            false
        }
    }

    pub(crate) fn open_hyperlink_uri(&self, processed_uri: &str) {
        #[cfg(not(any(target_os = "macos", windows)))]
        self.exec("xdg-open", [processed_uri]);

        #[cfg(target_os = "macos")]
        self.exec("open", [processed_uri]);

        #[cfg(windows)]
        self.exec("cmd", ["/c", "start", "", processed_uri]);
    }

    pub fn exec<I, S>(&self, program: &str, args: I)
    where
        I: IntoIterator<Item = S> + Debug + Copy,
        S: AsRef<OsStr>,
    {
        #[cfg(unix)]
        {
            let main_fd = *self.ctx().current().main_fd;
            let shell_pid = &self.ctx().current().shell_pid;
            match teletypewriter::spawn_daemon(program, args, main_fd, *shell_pid) {
                Ok(_) => tracing::debug!("Launched {} with args {:?}", program, args),
                Err(_) => {
                    tracing::warn!("Unable to launch {} with args {:?}", program, args)
                }
            }
        }

        #[cfg(windows)]
        {
            match teletypewriter::spawn_daemon(program, args) {
                Ok(_) => tracing::debug!("Launched {} with args {:?}", program, args),
                Err(_) => {
                    tracing::warn!("Unable to launch {} with args {:?}", program, args)
                }
            }
        }
    }

    pub fn contains_point(&self, x: usize, y: usize) -> bool {
        let current_grid = self.context_manager.current_grid();
        let (context, margin) = current_grid.current_context_with_computed_dimension();
        let layout = context.dimension;
        TerminalTextArea {
            // Margin is already pre-scaled (physical pixels), same as x/y.
            margin_left_px: margin.left,
            margin_top_px: margin.top,
            columns: layout.columns,
            lines: layout.lines,
            cell_width_px: layout.dimension.width,
            cell_height_px: layout.dimension.height,
        }
        .contains_point(x, y)
    }

    pub fn side_by_pos(&self, x: usize) -> Side {
        let current_grid = self.context_manager.current_grid();
        let (_, margin) = current_grid.current_context_with_computed_dimension();
        let current_context = self.context_manager.current();
        let layout = current_context.dimension;

        crate::input::mouse::calculate_side_by_pos(
            x,
            margin.left,
            layout.dimension.width,
            layout.width,
        )
    }

    pub fn selection_is_empty(&self) -> bool {
        self.context_manager
            .current()
            .renderable_content
            .selection_range
            .is_none()
    }
}
