use super::*;
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

impl Island {
    /// Toggle the color picker for a given tab index
    pub fn toggle_color_picker(&mut self, tab_index: usize, current_title: &str) {
        if self.color_picker_tab == Some(tab_index) {
            self.apply_rename();
            self.color_picker_tab = None;
        } else {
            self.color_picker_tab = Some(tab_index);
            // Initialize rename input with custom title or current displayed title
            self.rename_input = self
                .tab_custom_titles
                .get(&tab_index)
                .cloned()
                .unwrap_or_else(|| current_title.to_string());
            self.rename_caret_time = Instant::now();
        }
    }

    /// Close the color picker, applying any pending rename
    pub fn close_color_picker(&mut self) {
        if self.color_picker_tab.is_some() {
            self.apply_rename();
        }
        self.color_picker_tab = None;
    }

    pub fn remove_tab_state(&mut self, removed_index: usize) {
        self.tab_colors = std::mem::take(&mut self.tab_colors)
            .into_iter()
            .filter_map(|(idx, color)| {
                if idx < removed_index {
                    Some((idx, color))
                } else if idx > removed_index {
                    Some((idx - 1, color))
                } else {
                    None
                }
            })
            .collect();
        self.tab_custom_titles = std::mem::take(&mut self.tab_custom_titles)
            .into_iter()
            .filter_map(|(idx, title)| {
                if idx < removed_index {
                    Some((idx, title))
                } else if idx > removed_index {
                    Some((idx - 1, title))
                } else {
                    None
                }
            })
            .collect();
        self.color_picker_tab = self.color_picker_tab.and_then(|idx| {
            if idx < removed_index {
                Some(idx)
            } else if idx > removed_index {
                Some(idx - 1)
            } else {
                None
            }
        });
    }

    /// Apply the rename input as a custom title for the current picker tab
    pub(crate) fn apply_rename(&mut self) {
        if let Some(tab) = self.color_picker_tab {
            let trimmed = self.rename_input.trim().to_string();
            if trimmed.is_empty() {
                self.tab_custom_titles.remove(&tab);
            } else {
                self.tab_custom_titles.insert(tab, trimmed);
            }
        }
    }

    /// Handle keyboard input while the color picker (with rename field) is open.
    /// Returns true if input was consumed.
    ///
    /// TODO(wave-cutover): native called `handle_rename_input` with a
    /// `neoism_window::event::KeyEvent`. Shared mirrors the API on a POD
    /// [`IslandRenameKey`]; hosts translate winit events before calling.
    pub fn handle_rename_input(&mut self, key: IslandRenameKey) -> bool {
        if self.color_picker_tab.is_none() {
            return false;
        }

        match key {
            IslandRenameKey::Escape => {
                // Cancel — discard input, close picker
                self.color_picker_tab = None;
            }
            IslandRenameKey::Enter => {
                // Confirm — apply rename and close
                self.apply_rename();
                self.color_picker_tab = None;
            }
            IslandRenameKey::Backspace => {
                self.rename_input.pop();
                self.rename_caret_time = Instant::now();
            }
            IslandRenameKey::Character(ch) => {
                if !ch.is_control() {
                    self.rename_input.push(ch);
                    self.rename_caret_time = Instant::now();
                }
            }
        }
        true
    }

    /// Check if a click hits a color swatch in the picker.
    /// Returns true if the click was consumed.
    pub fn handle_color_picker_click(
        &mut self,
        mouse_x: f32,
        mouse_y: f32,
        scale_factor: f32,
        window_width: f32,
        num_tabs: usize,
    ) -> bool {
        let picker_tab = match self.color_picker_tab {
            Some(t) => t,
            None => return false,
        };

        let mouse_x_unscaled = mouse_x / scale_factor;
        let mouse_y_unscaled = mouse_y / scale_factor;

        // Compute the same tab layout as render()
        let left_margin = 0.0;

        let available_width =
            (window_width / scale_factor) - ISLAND_MARGIN_RIGHT - left_margin;
        let tab_width = available_width / num_tabs as f32;
        let tab_x = left_margin + picker_tab as f32 * tab_width;

        // Picker is rendered just below the island
        let picker_y = self.top_offset + ISLAND_HEIGHT;

        // Check if click is within picker vertical range
        if mouse_y_unscaled < picker_y || mouse_y_unscaled > picker_y + PICKER_HEIGHT {
            // Click outside picker — apply rename and close
            self.apply_rename();
            self.color_picker_tab = None;
            return false;
        }

        // Total picker width
        let total_swatches_width = PICKER_COLORS.len() as f32 * PICKER_SWATCH_SIZE
            + (PICKER_COLORS.len() - 1) as f32 * PICKER_SWATCH_GAP;
        let picker_start_x = tab_x + (tab_width - total_swatches_width) / 2.0;

        // Check each swatch
        let swatch_y = picker_y + PICKER_PADDING + PICKER_TOP_PADDING;
        let swatch_y_end = swatch_y + PICKER_SWATCH_SIZE;
        for (i, color) in PICKER_COLORS.iter().enumerate() {
            let swatch_x =
                picker_start_x + i as f32 * (PICKER_SWATCH_SIZE + PICKER_SWATCH_GAP);
            if mouse_x_unscaled >= swatch_x
                && mouse_x_unscaled <= swatch_x + PICKER_SWATCH_SIZE
                && mouse_y_unscaled >= swatch_y
                && mouse_y_unscaled <= swatch_y_end
            {
                self.tab_colors.insert(picker_tab, *color);
                self.apply_rename();
                self.color_picker_tab = None;
                return true;
            }
        }

        // Clicked in picker area but not on a swatch
        true
    }

    /// Render the color picker dropdown below a tab
    pub(crate) fn render_color_picker(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        tab_x: f32,
        tab_width: f32,
    ) {
        let padding = PICKER_PADDING;
        let bg_y = self.top_offset + ISLAND_HEIGHT;

        // Compute total swatches width to derive the consistent inner content width
        let total_swatches_width = PICKER_COLORS.len() as f32 * PICKER_SWATCH_SIZE
            + (PICKER_COLORS.len() - 1) as f32 * PICKER_SWATCH_GAP;
        let inner_width = total_swatches_width;
        let bg_width = inner_width + padding * 2.0;
        let bg_x = tab_x + (tab_width - bg_width) / 2.0;
        let content_x = bg_x + padding;

        // Background
        sugarloaf.rounded_rect(
            None,
            bg_x,
            bg_y,
            bg_width,
            PICKER_HEIGHT,
            [0.15, 0.15, 0.15, 1.0],
            0.0,
            4.0,
            10,
        );

        // Swatches — aligned to content_x
        let swatch_y = bg_y + padding + PICKER_TOP_PADDING;
        let picker_tab = self.color_picker_tab.unwrap_or(0);
        let selected_color = self.tab_colors.get(&picker_tab);
        for (i, color) in PICKER_COLORS.iter().enumerate() {
            let sx = content_x + i as f32 * (PICKER_SWATCH_SIZE + PICKER_SWATCH_GAP);
            let is_selected = selected_color == Some(color);

            // Draw white border behind selected swatch
            if is_selected {
                let border = 2.0;
                sugarloaf.rounded_rect(
                    None,
                    sx - border,
                    swatch_y - border,
                    PICKER_SWATCH_SIZE + border * 2.0,
                    PICKER_SWATCH_SIZE + border * 2.0,
                    [1.0, 1.0, 1.0, 1.0],
                    0.0,
                    4.0,
                    10,
                );
            }

            sugarloaf.rounded_rect(
                None,
                sx,
                swatch_y,
                PICKER_SWATCH_SIZE,
                PICKER_SWATCH_SIZE,
                *color,
                0.0,
                3.0,
                10,
            );
        }

        // Rename text input — same left/right edge as swatches
        let input_y = swatch_y + PICKER_SWATCH_SIZE + PICKER_INPUT_MARGIN_TOP;
        let input_x = content_x;
        let input_width = inner_width;

        // Input background
        sugarloaf.rounded_rect(
            None,
            input_x,
            input_y,
            input_width,
            PICKER_INPUT_HEIGHT,
            [0.10, 0.10, 0.10, 1.0],
            0.0,
            3.0,
            10,
        );

        let text_inset = 6.0;
        let text_x = input_x + text_inset;
        let max_text_width = input_width - text_inset * 2.0;
        let text_y = input_y + (PICKER_INPUT_HEIGHT - PICKER_INPUT_FONT_SIZE) / 2.0;

        let text_color = if self.rename_input.is_empty() {
            [0.45, 0.45, 0.45, 1.0]
        } else {
            [0.93, 0.93, 0.93, 1.0]
        };
        let rename_opts = DrawOpts {
            font_size: PICKER_INPUT_FONT_SIZE,
            color: color_u8(text_color),
            ..DrawOpts::default()
        };

        // Determine visible text: trim from the front if it overflows.
        let display_text: String = if self.rename_input.is_empty() {
            "Tab title...".to_string()
        } else {
            let input = self.rename_input.as_str();
            let chars: Vec<char> = input.chars().collect();
            let ui = sugarloaf.text_mut();
            let mut start = 0;
            let full_width = ui.measure(input, &rename_opts);
            if full_width > max_text_width {
                let mut lo = 0;
                let mut hi = chars.len();
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let substr: String = chars[mid..].iter().collect();
                    let w = ui.measure(&substr, &rename_opts);
                    if w > max_text_width {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                start = lo;
            }
            chars[start..].iter().collect()
        };

        let rendered_width =
            sugarloaf
                .text_mut()
                .draw(text_x, text_y, &display_text, &rename_opts);
        let rendered_width = if self.rename_input.is_empty() {
            0.0
        } else {
            rendered_width
        };

        // Blinking caret
        let elapsed = Instant::now()
            .saturating_duration_since(self.rename_caret_time)
            .as_millis();
        let show_caret = (elapsed / 500).is_multiple_of(2);
        if show_caret {
            let caret_x = text_x + rendered_width;
            if caret_x <= input_x + input_width {
                sugarloaf.rect(
                    None,
                    caret_x,
                    input_y + 4.0,
                    1.5,
                    PICKER_INPUT_HEIGHT - 8.0,
                    [0.93, 0.93, 0.93, 1.0],
                    0.0,
                    10,
                );
            }
        }
    }

    /// Whether the color picker is currently open
    pub fn is_color_picker_open(&self) -> bool {
        self.color_picker_tab.is_some()
    }

    /// Get the title text for a specific tab index
    pub(crate) fn get_title_for_tab(
        &self,
        contexts: &dyn IslandContexts,
        tab_index: usize,
    ) -> String {
        // Custom user-set title takes priority
        if let Some(custom) = self.tab_custom_titles.get(&tab_index) {
            return custom.clone();
        }

        if let Some(context_title) = contexts.title(tab_index) {
            return island_tab_label(
                &context_title.content,
                context_title.program.as_deref(),
            );
        }

        // Default fallback when there's no title or program yet.
        String::from("~")
    }
}
