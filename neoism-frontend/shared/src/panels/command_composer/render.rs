//! The per-frame `CommandComposer::render` pass plus a small handful of
//! drawing helpers that only the render path uses (`draw_text_thick`,
//! `draw_scramble_text_thick`, `line_clip`, `draw_chevrons`).

use web_time::Instant;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use super::classify::{
    draw_opts_for_style, line_for_byte, style_at, styled_spans, wrap_lines,
};
use super::shell_badge::{draw_shell_badge, shell_badge_label, shell_badge_width};
use super::state::CommandComposer;
use super::types::{
    ComposerFrame, InputClassification, InputWrapLayout, WrappedLine,
    CARET_BLINK_FALLBACK_MS, CHASSIS_RADIUS, CHIP_GAP, CHIP_PAD_X, CHIP_RADIUS,
    COMPOSER_MAX_INPUT_LINES, COMPOSER_TOP_OVERHANG, COMPOSER_WRAP_HARD_LIMIT, DEPTH,
    FAUX_BOLD_OFFSET, FONT_SIZE, HINT_FONT_SIZE, ORDER_CARET, ORDER_CHASSIS_BG,
    ORDER_CHASSIS_BORDER, ORDER_CHIP_BG, OUTER_PAD_X, PROMPT_BURST_MS, PROMPT_CHEVRONS,
    PROMPT_SCRAMBLE, SHELL_BADGE_FONT_SIZE, SHELL_SCRAMBLE, SHELL_TRANSITION_MS,
    SHOW_FOOTER_HINT_ROW,
};
use super::util::{color_u8_to_f32, hsl_to_u8, lerp_color_u8};
use crate::input::{CompletionFlashState, InputBuffer, TerminalShellKind};
use crate::primitives::IdeTheme;

/// status_line.rs measured this empirically for Geist Mono with a
/// 2.0*s lift on a 13*s font (= 0.654*em). Centering each glyph at
/// that ratio puts the `>` tip, typed-glyph midline, and caret
/// midline on the same y.
const CAP_CENTER_RATIO: f32 = 0.654;

pub(super) fn line_clip(x: f32, y: f32, width: f32, height: f32) -> [f32; 4] {
    [x, y - 3.0, width.max(0.0), height + 6.0]
}

pub(super) fn draw_text_thick(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    scale: f32,
) {
    sugarloaf.text_mut().draw(x, y, text, opts);
    sugarloaf
        .text_mut()
        .draw(x + FAUX_BOLD_OFFSET * scale, y, text, opts);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_scramble_text_thick(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    target: &str,
    base_opts: &DrawOpts,
    transition_t: f32,
    elapsed_ms: f32,
    animation_phase: f32,
    scale: f32,
) {
    let chars: Vec<char> = target.chars().collect();
    let count = chars.len().max(1) as f32;
    let frame = (elapsed_ms / 28.0) as usize;
    let mut cursor = x;
    for (idx, target_ch) in chars.iter().enumerate() {
        let lock_t = (idx as f32 + 1.0) / count;
        let locked = transition_t >= lock_t;
        let display = if locked {
            *target_ch
        } else {
            let scramble_ix = (frame + idx * 7) % SHELL_SCRAMBLE.len();
            SHELL_SCRAMBLE[scramble_ix] as char
        };
        let hue = (animation_phase * 190.0 + idx as f32 * 48.0).rem_euclid(360.0);
        let opts = DrawOpts {
            color: if locked {
                base_opts.color
            } else {
                hsl_to_u8(hue, 1.0, 0.68)
            },
            ..*base_opts
        };
        let mut buf = [0u8; 4];
        let glyph = display.encode_utf8(&mut buf);
        draw_text_thick(sugarloaf, cursor, y, glyph, &opts, scale);
        let mut target_buf = [0u8; 4];
        let target_glyph = target_ch.encode_utf8(&mut target_buf);
        cursor += sugarloaf.text_mut().measure(target_glyph, base_opts);
    }
}

impl CommandComposer {
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        width: f32,
        chassis_h_logical: f32,
        theme: &IdeTheme,
        input: &dyn InputBuffer,
        cwd_label: Option<&str>,
        animation_phase: f32,
        focused: bool,
        cell_w: f32,
        cell_h: f32,
        trail_cursor_will_paint: bool,
        cursor_blink_interval_ms: u64,
        classification: InputClassification,
        shell_kind: TerminalShellKind,
    ) -> ComposerFrame {
        if !self.visible || width <= 0.0 {
            self.last_frame = ComposerFrame::default();
            self.completion_popup_rect = None;
            self.last_input_wrap = None;
            return self.last_frame;
        }
        let now = Instant::now();
        self.last_render = now;
        // Reset caret blink phase whenever the user just typed — Warp
        // shows a solid caret right after a keypress, then resumes
        // blinking. Tracking text/cursor changes is cheap enough to do
        // every frame.
        let cursor_byte = input.cursor_byte();
        if input.text().len() != self.last_text_len
            || cursor_byte != self.last_cursor_byte
        {
            self.last_caret_seen = now;
            self.last_text_len = input.text().len();
            self.last_cursor_byte = cursor_byte;
        }
        match self.last_shell_kind {
            None => {
                self.last_shell_kind = Some(shell_kind);
                self.previous_shell_kind = shell_kind;
            }
            Some(current) if current != shell_kind => {
                self.previous_shell_kind = current;
                self.last_shell_kind = Some(shell_kind);
                self.shell_transition_started = now;
            }
            _ => {}
        }

        let s = self.scale;
        // Chassis spans exactly the reserved cell band (caller picks
        // chassis_h_logical = reserved_rows × cell_h_logical so the
        // chassis fills its row reservation with zero gap above).
        // No outer ribbon — the cleared cells around the chassis show
        // the terminal background on every side, giving the floating
        // island look.
        let chassis_x = x_left + OUTER_PAD_X * s;
        let chassis_y = y_top;
        let chassis_w = (width - OUTER_PAD_X * 2.0 * s).max(0.0);
        let chassis_inner_h = chassis_h_logical.max(2.0);
        let composer_top_y = (chassis_y - COMPOSER_TOP_OVERHANG * s).max(0.0);
        let composer_extra_top = chassis_y - composer_top_y;
        let composer_top_radius = CHASSIS_RADIUS * s;
        let chip_fill = theme.f32(theme.surface);

        // Edge-to-edge: surface fill sits under the antialiased border
        // so the rounded command corners don't pick up dark fringe
        // pixels. The visual lip can float above the reserved rows,
        // but it no longer costs a whole blank terminal row.
        let _ = focused; // focus signal lives on the chips for now
        sugarloaf.quad(
            None,
            chassis_x,
            composer_top_y,
            chassis_w,
            chassis_inner_h + composer_extra_top,
            chip_fill,
            [composer_top_radius, composer_top_radius, 0.0, 0.0],
            DEPTH,
            ORDER_CHASSIS_BG,
        );
        let inset = 0.0;

        // ── Row geometry ─────────────────────────────────────────────
        // Top row (chip + prompt + send chip) gets the upper ~70%; hint
        // line sits in the lower ~30%. Optical lift on the body baseline
        // matches the trick `status_line.rs` uses — Geist Mono + nerd
        // glyphs land slightly low without it.
        let font_size = FONT_SIZE * s;
        let hint_size = HINT_FONT_SIZE * s;
        let raw_text = input.text();
        let cursor_byte = input.cursor_byte().min(raw_text.len());
        let suggestion = input.suggestion_after_cursor().unwrap_or("");
        let completion_items = input.completion_items();
        if completion_items.len() != self.last_completion_count {
            self.completion_popup_started = now;
            self.last_completion_count = completion_items.len();
            self.reset_completion_motion();
        }
        if completion_items.is_empty() {
            self.completion_popup_rect = None;
        }
        let line_step = if cell_h > 0.0 {
            cell_h.max(font_size + 3.0 * s)
        } else {
            font_size + 5.0 * s
        };
        let caret_h = if cell_h > 0.0 {
            cell_h
        } else {
            font_size + 4.0 * s
        };
        let control_font_size = (caret_h - 2.0 * s).max(font_size * 1.18);
        let chevron_font_size = control_font_size * 1.10;
        let estimated_lines =
            self.estimated_input_line_count(chassis_w, cell_w.max(1.0), raw_text);
        let body_rows_h = estimated_lines as f32 * line_step;
        let footer_reserved_h = if SHOW_FOOTER_HINT_ROW {
            hint_size + 5.5 * s
        } else {
            0.0
        };
        let max_body_h = (chassis_inner_h - footer_reserved_h).max(line_step);
        let row_split = body_rows_h.min(max_body_h).max(line_step);
        let hint_gap = if SHOW_FOOTER_HINT_ROW { 5.5 * s } else { 0.0 };
        let group_h = row_split + footer_reserved_h;
        let group_top = chassis_y + ((chassis_inner_h - group_h).max(0.0) * 0.50);
        let control_row_lift = 2.5 * s;
        let hint_y = (group_top + row_split + hint_gap)
            .min(chassis_y + chassis_inner_h - hint_size - 4.0 * s);

        let inner_left = chassis_x + CHIP_PAD_X * s + inset;
        let inner_right = chassis_x + chassis_w - CHIP_PAD_X * s - inset;
        let control_chip_pad_x = 10.0 * s;

        // ── cwd chip (commented out per user request) ───────────────
        // let cwd_text = cwd_label.unwrap_or("~");
        // let cwd_opts = DrawOpts {
        //     font_size: control_font_size,
        //     color: theme.u8(theme.fg),
        //     bold: true,
        //     ..DrawOpts::default()
        // };
        // let cwd_text_w = sugarloaf.text_mut().measure(cwd_text, &cwd_opts);
        // let cwd_chip_w = cwd_text_w + control_chip_pad_x * 2.0;
        let _ = cwd_label;
        let cwd_chip_w: f32 = 0.0;
        let chip_h = caret_h.max(control_font_size + 2.0 * s);
        let chip_y = group_top + (line_step - chip_h) / 2.0 + inset - control_row_lift;
        // Single row center shared by the chevron tip, caret midline, and
        // typed-glyph midline. Sugarloaf takes `y` as the top of the
        // ascent line, so the *visual* cap-height midline of a drawn
        // glyph sits BELOW the em-box midline.
        let row_center_y = chip_y + chip_h * 0.5;
        let body_y = row_center_y - font_size * CAP_CENTER_RATIO;
        let control_y = row_center_y - control_font_size * CAP_CENTER_RATIO;
        // Chips sit a notch above the chassis bg so they read as
        // discrete pills. `theme.surface` is the standard "raised
        // chrome" tone in every theme — works on every palette without
        // hardcoding a single grey.
        let command_plate_stroke = (2.25 * s).max(2.0);
        let command_plate_radius = composer_top_radius;
        let command_plate_y = composer_top_y;
        // The status strip starts painting a hair above its own top
        // edge to hide seams, so extend the rounded plate itself a
        // couple pixels into that join. The inset bg below still covers
        // the center down to the bottom, leaving only the side rails
        // visible there; no separate square-ended rail rects.
        let rail_join_overlap = (6.0 * s).max(3.0);
        let command_plate_h = chassis_inner_h + composer_extra_top + rail_join_overlap;
        sugarloaf.quad(
            None,
            chassis_x,
            command_plate_y,
            chassis_w,
            command_plate_h,
            chip_fill,
            [command_plate_radius, command_plate_radius, 0.0, 0.0],
            DEPTH,
            ORDER_CHASSIS_BORDER,
        );
        sugarloaf.quad(
            None,
            chassis_x + command_plate_stroke,
            command_plate_y + command_plate_stroke,
            (chassis_w - command_plate_stroke * 2.0).max(0.0),
            (command_plate_h - command_plate_stroke).max(0.0),
            theme.f32(theme.bg),
            [
                (command_plate_radius - command_plate_stroke).max(0.0),
                (command_plate_radius - command_plate_stroke).max(0.0),
                0.0,
                0.0,
            ],
            DEPTH,
            ORDER_CHASSIS_BORDER + 1,
        );

        // ── Send chip (right side) ──────────────────────────────────
        // Always uses the dark chip fill; only the text color +
        // optional accent border carry the active/idle distinction.
        // Theme `accent` on `pastel_dark` is `#e8e8e8` — using it as a
        // chip background made the corner read "almost white". Putting
        // the accent in the text instead (with a 1px ring on the chip
        // when active) keeps the affordance loud without the glare.
        let send_label = "run";
        let send_active = !input.is_empty();
        let send_text_color = if send_active {
            theme.u8(theme.accent)
        } else {
            theme.u8_alpha(theme.muted, 0.85)
        };
        let send_opts = DrawOpts {
            font_size: control_font_size,
            color: send_text_color,
            bold: true,
            ..DrawOpts::default()
        };
        let send_icon_w = 13.0 * s;
        let send_icon_gap = 5.0 * s;
        let send_text_w = sugarloaf.text_mut().measure(send_label, &send_opts);
        let send_chip_w =
            send_icon_w + send_icon_gap + send_text_w + control_chip_pad_x * 2.0;
        let send_chip_x = inner_right - send_chip_w;
        // 1px accent ring under the chip when there's text to submit —
        // reads as "armed" without painting a big colored pill.
        if send_active {
            sugarloaf.rounded_rect(
                None,
                send_chip_x,
                chip_y,
                send_chip_w,
                chip_h,
                theme.f32_alpha(theme.accent, 0.55),
                DEPTH,
                CHIP_RADIUS * s,
                ORDER_CHIP_BG,
            );
            sugarloaf.rounded_rect(
                None,
                send_chip_x + 1.0 * s,
                chip_y + 1.0 * s,
                (send_chip_w - 2.0 * s).max(0.0),
                (chip_h - 2.0 * s).max(0.0),
                chip_fill,
                DEPTH,
                (CHIP_RADIUS * s - 1.0 * s).max(0.0),
                ORDER_CHIP_BG + 1,
            );
        } else {
            sugarloaf.rounded_rect(
                None,
                send_chip_x,
                chip_y,
                send_chip_w,
                chip_h,
                chip_fill,
                DEPTH,
                CHIP_RADIUS * s,
                ORDER_CHIP_BG,
            );
        }
        let icon_color = color_u8_to_f32(send_text_color);
        let icon_x = send_chip_x + control_chip_pad_x;
        let icon_h = 12.0 * s;
        let icon_y = chip_y + (chip_h - icon_h) * 0.5;
        let stroke = (1.7 * s).max(1.0);
        let joint_y = icon_y + icon_h * 0.62;
        let stem_x = icon_x + send_icon_w - stroke * 0.5;
        let arrow_tip_x = icon_x + 0.5 * s;
        let arrow_base_x = icon_x + 5.3 * s;

        sugarloaf.rounded_rect(
            None,
            arrow_base_x,
            joint_y - stroke * 0.5,
            (stem_x - arrow_base_x).max(stroke),
            stroke,
            icon_color,
            DEPTH,
            stroke * 0.5,
            ORDER_CARET,
        );
        sugarloaf.rounded_rect(
            None,
            stem_x - stroke * 0.5,
            icon_y + 1.5 * s,
            stroke,
            (joint_y - icon_y - 1.5 * s).max(stroke),
            icon_color,
            DEPTH,
            stroke * 0.5,
            ORDER_CARET,
        );
        sugarloaf.triangle_ordered(
            arrow_tip_x,
            joint_y,
            arrow_base_x,
            joint_y - 4.0 * s,
            arrow_base_x,
            joint_y + 4.0 * s,
            DEPTH,
            icon_color,
            ORDER_CARET,
        );
        sugarloaf.text_mut().draw(
            icon_x + send_icon_w + send_icon_gap,
            control_y,
            send_label,
            &send_opts,
        );

        // ── `>>>` chevrons (animated rainbow on burst) ──────────────
        // Advance uses the same fixed monospace-grid math as
        // `draw_chevrons`, computed up front so wrap widths stay
        // identical whether or not the prompt is painted this frame —
        // it hides while the input window is scrolled (the top chassis
        // row then belongs to chrome only: run chip + hidden-rows
        // indicator). The draw itself happens after the window
        // position is known.
        let burst = input.prompt_burst_elapsed_ms();
        let prompt_x = inner_left + cwd_chip_w + CHIP_GAP * s;
        let chevron_y = row_center_y - chevron_font_size * CAP_CENTER_RATIO;
        let chevron_advance =
            PROMPT_CHEVRONS as f32 * (chevron_font_size * 0.62) + 6.0 * s;

        // ── Editable text + history-suggestion ghost ────────────────
        let text_x = prompt_x + chevron_advance + 2.0 * s;
        let first_text_avail = (send_chip_x - 6.0 * s - text_x).max(0.0);
        // Continuation rows get the FULL inner width — only the first
        // chassis row shares its line with the run chip. (Clamping
        // every wrapped row at the run-chip column left a huge dead
        // right margin on multi-line drafts.)
        let wrapped_text_avail = (inner_right - inner_left).max(first_text_avail);
        self.last_input_wrap = Some(InputWrapLayout {
            first_width: first_text_avail,
            wrapped_width: wrapped_text_avail,
            cell_width: cell_w.max(1.0),
        });
        let body_clip =
            Some([text_x, chassis_y, wrapped_text_avail, row_split.max(chip_h)]);
        let ghost_opts = DrawOpts {
            font_size,
            color: theme.u8_alpha(theme.muted, 0.85),
            bold: false,
            clip_rect: body_clip,
            ..DrawOpts::default()
        };

        // Tab-feedback flash: composer reads the latest completion
        // outcome each frame. Success paints a fading accent rect
        // over the newly-inserted byte range; NoMatch tints the whole
        // text red and shakes it horizontally on a decaying sine.
        let flash = input.flash_state();
        let (shake_offset, no_match_intensity) = match flash {
            Some(CompletionFlashState::NoMatch {
                shake_offset_logical,
                intensity,
            }) => (shake_offset_logical * s, intensity),
            _ => (0.0, 0.0),
        };
        // Mix red into the command/args color while a NoMatch flash is
        // alive — fades back to the classifier color as intensity → 0.
        let red = theme.u8(theme.red);
        let mut spans = styled_spans(raw_text, classification);
        for span in &mut spans {
            span.style.color = lerp_color_u8(span.style.color, red, no_match_intensity);
        }
        let max_input_lines =
            ((row_split - 2.0 * s) / line_step).floor().max(1.0) as usize;
        let max_input_lines = max_input_lines.clamp(1, COMPOSER_MAX_INPUT_LINES);
        let wrapped_lines = wrap_lines(
            raw_text,
            first_text_avail,
            wrapped_text_avail,
            cell_w.max(1.0),
            COMPOSER_WRAP_HARD_LIMIT,
        );
        let cursor_line_global = line_for_byte(&wrapped_lines, cursor_byte);
        let mut visible_line_count = max_input_lines.min(wrapped_lines.len().max(1));
        let mut first_visible_line = cursor_line_global
            .saturating_add(1)
            .saturating_sub(visible_line_count);
        // Scrolled window: the top chassis row (chevron prompt + run
        // chip) becomes chrome-only — the prompt hides, text starts
        // one display slot down, and continuation rows never collide
        // with the chip. Costs one visible row while scrolled.
        let window_at_top = first_visible_line == 0 || max_input_lines <= 1;
        let row_slot_offset: usize = if window_at_top { 0 } else { 1 };
        if !window_at_top {
            visible_line_count = max_input_lines.saturating_sub(1).max(1);
            first_visible_line = cursor_line_global
                .saturating_add(1)
                .saturating_sub(visible_line_count);
        }
        let last_visible_line =
            (first_visible_line + visible_line_count).min(wrapped_lines.len());
        let lines = wrapped_lines[first_visible_line..last_visible_line].to_vec();
        let cursor_line = cursor_line_global.saturating_sub(first_visible_line);
        if window_at_top {
            self.draw_chevrons(
                sugarloaf,
                prompt_x,
                chevron_y,
                chevron_font_size,
                burst,
                animation_phase,
                theme,
            );
        }
        let cursor_line_start = lines
            .get(cursor_line)
            .map(|line| line.start)
            .unwrap_or(0)
            .min(cursor_byte);
        let cursor_before = &raw_text[cursor_line_start..cursor_byte];
        let cursor_style = style_at(&spans, cursor_byte).unwrap_or(classification.arg);
        let cursor_opts = draw_opts_for_style(
            font_size,
            cursor_style,
            Some(line_clip(
                if window_at_top && cursor_line == 0 {
                    text_x
                } else {
                    inner_left
                },
                body_y + (row_slot_offset + cursor_line) as f32 * line_step,
                lines
                    .get(cursor_line)
                    .map(|line| line.width_limit)
                    .unwrap_or(wrapped_text_avail),
                line_step,
            )),
        );
        let before_w = if cursor_before.is_empty() {
            0.0
        } else {
            sugarloaf.text_mut().measure(cursor_before, &cursor_opts)
        };

        let mut suggestion_x = text_x;
        let mut suggestion_y = body_y;
        for (line_idx, line) in lines.iter().enumerate() {
            let line_y = body_y + (row_slot_offset + line_idx) as f32 * line_step;
            let line_x = if window_at_top && line_idx == 0 {
                text_x
            } else {
                inner_left
            };
            let clip = Some(line_clip(line_x, line_y, line.width_limit, line_step));
            let mut draw_x = line_x + shake_offset;
            for span in spans
                .iter()
                .filter(|span| span.end > line.start && span.start < line.end)
            {
                let start = span.start.max(line.start);
                let end = span.end.min(line.end);
                if start >= end {
                    continue;
                }
                let text = &raw_text[start..end];
                let opts = draw_opts_for_style(font_size, span.style, clip);
                sugarloaf.text_mut().draw(draw_x, line_y, text, &opts);
                let w = sugarloaf.text_mut().measure(text, &opts);
                if span.style.underline {
                    sugarloaf.rect(
                        None,
                        draw_x,
                        line_y + font_size + 1.5 * s,
                        w,
                        (1.5 * s).max(1.0),
                        color_u8_to_f32(span.style.color),
                        DEPTH,
                        ORDER_CARET,
                    );
                }
                draw_x += w;
            }
            if cursor_byte == line.end {
                suggestion_x = draw_x;
                suggestion_y = line_y;
            }
        }
        if raw_text.is_empty() {
            suggestion_x = text_x;
            suggestion_y = body_y;
        }
        if !suggestion.is_empty() && cursor_byte == raw_text.len() {
            let line = lines.last().copied().unwrap_or(WrappedLine {
                start: 0,
                end: 0,
                width_limit: first_text_avail,
            });
            let line_idx = lines.len().saturating_sub(1);
            let line_x = if window_at_top && line_idx == 0 {
                text_x
            } else {
                inner_left
            };
            let clip = Some(line_clip(line_x, suggestion_y, line.width_limit, line_step));
            let opts = DrawOpts {
                clip_rect: clip,
                ..ghost_opts
            };
            sugarloaf
                .text_mut()
                .draw(suggestion_x, suggestion_y, suggestion, &opts);
        }

        // Hidden-row indicators: when the input window is scrolled
        // inside a large draft/paste, say how many wrapped rows sit
        // outside the visible band — content must never silently
        // disappear above/below the window. Each label gets a small
        // surface-colored pill under it so it stays readable when a
        // full-width text row runs underneath.
        {
            let hidden_above = first_visible_line;
            let hidden_below = wrapped_lines.len().saturating_sub(last_visible_line);
            if hidden_above > 0 || hidden_below > 0 {
                let indicator_opts = DrawOpts {
                    font_size: (font_size * 0.72).max(8.0),
                    color: theme.u8_alpha(theme.muted, 0.9),
                    bold: true,
                    ..DrawOpts::default()
                };
                let right_edge = send_chip_x - 10.0 * s;
                let draw_indicator =
                    |sugarloaf: &mut Sugarloaf, label: String, y: f32| {
                        let w = sugarloaf.text_mut().measure(&label, &indicator_opts);
                        let x = (right_edge - w).max(inner_left);
                        let pill_pad = 5.0 * s;
                        let pill_h = indicator_opts.font_size + 5.0 * s;
                        sugarloaf.rounded_rect(
                            None,
                            x - pill_pad,
                            y - 2.5 * s,
                            w + pill_pad * 2.0,
                            pill_h,
                            theme.f32_alpha(theme.surface, 0.95),
                            DEPTH,
                            pill_h * 0.4,
                            ORDER_CHIP_BG,
                        );
                        sugarloaf.text_mut().draw(x, y, &label, &indicator_opts);
                    };
                if hidden_above > 0 {
                    // Slot 0 is chrome-only while scrolled, so the
                    // "above" indicator lives there beside the run chip.
                    draw_indicator(sugarloaf, format!("↑ {hidden_above} more"), body_y);
                }
                if hidden_below > 0 {
                    let y = body_y
                        + (row_slot_offset + visible_line_count.saturating_sub(1)) as f32
                            * line_step;
                    draw_indicator(sugarloaf, format!("↓ {hidden_below} more"), y);
                }
            }
        }

        // Success flash overlay — fading accent-tinted highlight on
        // the inserted byte range. Painted UNDER the text by drawing
        // it with a low order before the text's order. Sugarloaf
        // doesn't expose explicit z, so we use rect ORDER_CHIP_BG
        // (below the text default) and let alpha do the rest.
        if let Some(CompletionFlashState::Success { range, intensity }) = flash {
            let (start, end) = range;
            let start = start.min(raw_text.len());
            let end = end.min(raw_text.len()).max(start);
            if end > start {
                let line_idx_global = line_for_byte(&wrapped_lines, start);
                if line_idx_global >= first_visible_line
                    && line_idx_global < last_visible_line
                {
                    let span = &raw_text[start..end];
                    let line_idx = line_idx_global - first_visible_line;
                    let line = lines.get(line_idx).copied().unwrap_or(WrappedLine {
                        start: 0,
                        end: raw_text.len(),
                        width_limit: first_text_avail,
                    });
                    let pre = &raw_text[line.start..start];
                    let style = style_at(&spans, start).unwrap_or(classification.arg);
                    let opts = draw_opts_for_style(font_size, style, None);
                    let pre_w = sugarloaf.text_mut().measure(pre, &opts);
                    let span_w = sugarloaf.text_mut().measure(span, &opts);
                    let line_x = if window_at_top && line_idx == 0 {
                        text_x
                    } else {
                        inner_left
                    };
                    let highlight_h = font_size + 4.0 * s;
                    let highlight_y = body_y
                        + (row_slot_offset + line_idx) as f32 * line_step
                        - 2.0 * s;
                    sugarloaf.rounded_rect(
                        None,
                        line_x + shake_offset + pre_w - 2.0 * s,
                        highlight_y,
                        span_w + 4.0 * s,
                        highlight_h,
                        theme.f32_alpha(theme.green, 0.22 * intensity),
                        DEPTH,
                        3.0 * s,
                        ORDER_CHIP_BG,
                    );
                }
            }
        }

        // ── Caret ───────────────────────────────────────────────────
        // Match the cell-grid cursor — a Block sized to one terminal
        // cell, in the configured cursor color. When trail_cursor is
        // enabled the system-wide spring animator paints the actual
        // pixels (we just hand back the destination rect via
        // `caret_rect`); otherwise we draw the block directly with the
        // standard cursor blink interval so the composer caret pulses
        // in lockstep with the editor caret elsewhere in the IDE.
        let caret_w = if cell_w > 0.0 {
            cell_w
        } else {
            (font_size * 0.6).max(4.0 * s)
        };
        let cursor_line_width = lines
            .get(cursor_line)
            .map(|line| line.width_limit)
            .unwrap_or(wrapped_text_avail);
        let cursor_line_x = if window_at_top && cursor_line == 0 {
            text_x
        } else {
            inner_left
        };
        let caret_x = (cursor_line_x + before_w + shake_offset)
            .min(cursor_line_x + cursor_line_width - caret_w)
            .max(cursor_line_x);
        let caret_y = chip_y
            + (chip_h - caret_h) * 0.5
            + (row_slot_offset + cursor_line) as f32 * line_step;
        let caret_rect = if focused {
            if !trail_cursor_will_paint {
                let blink_ms = if cursor_blink_interval_ms == 0 {
                    CARET_BLINK_FALLBACK_MS
                } else {
                    cursor_blink_interval_ms as f32
                };
                let elapsed_ms = now
                    .saturating_duration_since(self.last_caret_seen)
                    .as_secs_f32()
                    * 1000.0;
                let on = (elapsed_ms / blink_ms) as u64 % 2 == 0;
                if on {
                    sugarloaf.rect(
                        None,
                        caret_x,
                        caret_y,
                        caret_w,
                        caret_h,
                        theme.f32(theme.accent),
                        DEPTH,
                        ORDER_CARET,
                    );
                }
            }
            Some([caret_x, caret_y, caret_w, caret_h])
        } else {
            // Hollow underline when the pane is unfocused — the
            // composer is parked, so a full block would shout. One
            // pixel of accent at the bottom of the cell hints at where
            // typing would resume.
            sugarloaf.rect(
                None,
                caret_x,
                caret_y + caret_h - 1.0 * s,
                caret_w,
                1.0 * s,
                theme.f32_alpha(theme.muted, 0.7),
                DEPTH,
                ORDER_CARET,
            );
            None
        };

        if SHOW_FOOTER_HINT_ROW {
            // ── Footer shell badge + global shortcut hints ──────────
            let key_color = theme.u8_alpha(theme.dim, 0.9);
            let word_color = theme.u8_alpha(theme.muted, 0.95);
            let dot_color = theme.u8_alpha(theme.border, 1.0);
            let key_opts = DrawOpts {
                font_size: hint_size,
                color: key_color,
                bold: true,
                ..DrawOpts::default()
            };
            let word_opts = DrawOpts {
                font_size: hint_size,
                color: word_color,
                ..DrawOpts::default()
            };
            let dot_opts = DrawOpts {
                font_size: hint_size,
                color: dot_color,
                bold: true,
                ..DrawOpts::default()
            };
            let segments: &[(&str, &str)] = &[("Alt+P", "command"), ("Alt+S", "search")];
            let space_w = sugarloaf.text_mut().measure(" ", &word_opts);
            let dot_w = sugarloaf.text_mut().measure(" · ", &dot_opts);
            let total_w = segments
                .iter()
                .enumerate()
                .map(|(i, (key, word))| {
                    let separator_w = if i == 0 { 0.0 } else { dot_w };
                    separator_w
                        + sugarloaf.text_mut().measure(key, &key_opts)
                        + space_w
                        + sugarloaf.text_mut().measure(word, &word_opts)
                })
                .sum::<f32>();
            let shortcut_x = (inner_right - total_w).max(inner_left);
            let shell_label = shell_badge_label(shell_kind);
            let shell_font_size = SHELL_BADGE_FONT_SIZE * s;
            let target_shell_pill_h = (shell_font_size + 5.0 * s).max(14.0 * s);
            let shell_top_gap = 5.0 * s;
            let shell_top_min = chip_y + chip_h + shell_top_gap;
            let shell_bottom_max = chassis_y + chassis_inner_h - 1.0 * s;
            let shell_available_h = shell_bottom_max - shell_top_min;
            let shell_pill_h = if shell_available_h >= shell_font_size + 3.0 * s {
                target_shell_pill_h.min(shell_available_h)
            } else {
                target_shell_pill_h
            };
            let shell_pill_y = (hint_y + 1.5 * s)
                .max(shell_top_min)
                .min(shell_bottom_max - shell_pill_h)
                .max(chassis_y + 1.0 * s);
            let shell_body_y =
                shell_pill_y + (shell_pill_h - shell_font_size) / 2.0 - 2.0 * s;
            let shell_badge_w =
                shell_badge_width(sugarloaf, shell_font_size, shell_label, s);
            let shell_x = inner_left;
            if shell_badge_w > 0.0 && shell_x + shell_badge_w + 4.0 * s <= shortcut_x {
                let transition_elapsed_ms = now
                    .saturating_duration_since(self.shell_transition_started)
                    .as_secs_f32()
                    * 1000.0;
                let transition_t =
                    (transition_elapsed_ms / SHELL_TRANSITION_MS).clamp(0.0, 1.0);
                draw_shell_badge(
                    sugarloaf,
                    shell_x,
                    shell_pill_y,
                    shell_pill_h,
                    shell_body_y,
                    shell_font_size,
                    shell_label,
                    shell_kind,
                    self.previous_shell_kind,
                    transition_t,
                    transition_elapsed_ms,
                    animation_phase,
                    theme,
                    s,
                );
            }
            if let Some(notice) = input.control_notice() {
                let notice_opts = DrawOpts {
                    font_size: hint_size,
                    color: theme.u8(theme.red),
                    bold: true,
                    ..DrawOpts::default()
                };
                let notice_x = shell_x + shell_badge_w + 10.0 * s;
                let notice_w = sugarloaf.text_mut().measure(notice, &notice_opts);
                if notice_x + notice_w + 8.0 * s <= shortcut_x {
                    sugarloaf
                        .text_mut()
                        .draw(notice_x, hint_y, notice, &notice_opts);
                }
            }

            let mut hx = shortcut_x;
            for (i, (key, word)) in segments.iter().enumerate() {
                if i > 0 {
                    sugarloaf.text_mut().draw(hx, hint_y, " · ", &dot_opts);
                    hx += dot_w;
                }
                let kw = sugarloaf.text_mut().measure(key, &key_opts);
                sugarloaf.text_mut().draw(hx, hint_y, key, &key_opts);
                hx += kw + space_w;
                let ww = sugarloaf.text_mut().measure(word, &word_opts);
                sugarloaf.text_mut().draw(hx, hint_y, word, &word_opts);
                hx += ww;
            }
        }

        if !completion_items.is_empty() {
            let scale_factor = sugarloaf.scale_factor();
            self.draw_completion_popup(
                sugarloaf,
                completion_items,
                input.completion_detail(),
                text_x,
                composer_top_y,
                chassis_x,
                chassis_w,
                theme,
                s,
                scale_factor,
                now.saturating_duration_since(self.completion_popup_started)
                    .as_secs_f32()
                    * 1000.0,
            );
        }

        let frame = ComposerFrame {
            chassis_rect: [chassis_x, chassis_y, chassis_w, chassis_inner_h],
            caret_rect,
            send_chip_rect: [send_chip_x, chip_y, send_chip_w, chip_h],
        };
        self.last_frame = frame;
        frame
    }

    /// Animated `>>>` prefix — three chevrons that scramble through
    /// punctuation and "lock" sequentially during the prompt burst,
    /// fading from rainbow → bold cyan as the burst settles. Returns
    /// the total advance so the caller can position text after it.
    #[allow(clippy::too_many_arguments)]
    fn draw_chevrons(
        &self,
        sugarloaf: &mut Sugarloaf,
        x: f32,
        y: f32,
        font_size: f32,
        burst_elapsed_ms: Option<f32>,
        animation_phase: f32,
        theme: &IdeTheme,
    ) -> f32 {
        let s = self.scale;
        let burst = burst_elapsed_ms
            .map(|elapsed| 1.0 - (elapsed / PROMPT_BURST_MS).clamp(0.0, 1.0))
            .unwrap_or(0.0);
        let frame = (animation_phase * 38.0) as usize;
        let mut cursor = x;
        for idx in 0..PROMPT_CHEVRONS {
            let lock_threshold = (idx as f32 + 1.0) / PROMPT_CHEVRONS as f32;
            let unlocked_t = burst.max(0.0);
            let locked = burst_elapsed_ms
                .map(|elapsed| {
                    let t = (elapsed / PROMPT_BURST_MS).clamp(0.0, 1.0);
                    t >= lock_threshold
                })
                .unwrap_or(true);

            let display = if locked {
                // Nerd Font rounded chevron-right (FontAwesome `chevron-right`).
                '\u{f054}'
            } else {
                let scramble_ix = (frame + idx * 5) % PROMPT_SCRAMBLE.len();
                PROMPT_SCRAMBLE[scramble_ix] as char
            };
            let speed = 160.0 + unlocked_t * 320.0;
            let hue = (animation_phase * speed + idx as f32 * 54.0).rem_euclid(360.0);
            let color = if locked && burst <= 0.0 {
                theme.u8(theme.accent)
            } else {
                hsl_to_u8(hue, 1.0, 0.64 + unlocked_t * 0.12)
            };
            let opts = DrawOpts {
                font_size,
                color,
                bold: true,
                ..DrawOpts::default()
            };
            let mut buf = [0u8; 4];
            let glyph = display.encode_utf8(&mut buf);
            sugarloaf.text_mut().draw(cursor, y, glyph, &opts);
            // Manual fixed advance so all three chevrons land on a
            // monospace grid even when the scramble glyph (e.g. `|`)
            // measures narrower than `>`. Keeps the chevron row from
            // visibly jittering as characters lock in.
            let advance = font_size * 0.62;
            cursor += advance;
        }
        cursor - x + 6.0 * s
    }
}
