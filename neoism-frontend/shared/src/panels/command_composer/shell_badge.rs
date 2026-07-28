//! Shell-detect pill drawn in the footer hint row.
//!
//! Renders a rounded badge with an icon section (filled with the
//! shell's accent color) and a label section (transparent surface with
//! the accent as text). Animates between shells via a per-glyph
//! scramble pass that locks in left-to-right.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use super::render::{draw_scramble_text_thick, draw_text_thick};
use super::types::{DEPTH, GLYPH_SHELL, ORDER_CHIP_BG};
use super::util::{color_u8_to_f32, ease_out_cubic, lerp_color_f32, lerp_color_u8};
use crate::input::TerminalShellKind;
use crate::primitives::IdeTheme;

pub(super) fn shell_badge_label(kind: TerminalShellKind) -> &'static str {
    match kind {
        TerminalShellKind::Bash => "Bash",
        TerminalShellKind::Zsh => "Zsh",
        TerminalShellKind::Fish => "Fish",
        TerminalShellKind::PowerShell => "PowerShell",
        TerminalShellKind::Cmd => "Cmd",
        TerminalShellKind::Unknown => "Shell",
    }
}

pub(super) fn shell_badge_accent(kind: TerminalShellKind, theme: &IdeTheme) -> [u8; 4] {
    match kind {
        TerminalShellKind::Bash => theme.u8(theme.green),
        TerminalShellKind::Zsh => theme.u8(theme.cyan),
        TerminalShellKind::Fish => theme.u8(theme.yellow),
        TerminalShellKind::PowerShell => theme.u8(theme.blue),
        TerminalShellKind::Cmd => theme.u8(theme.green),
        TerminalShellKind::Unknown => theme.u8(theme.magenta),
    }
}

pub(super) fn shell_badge_width(
    sugarloaf: &mut Sugarloaf,
    font_size: f32,
    label: &str,
    scale: f32,
) -> f32 {
    let opts = DrawOpts {
        font_size,
        bold: true,
        ..DrawOpts::default()
    };
    let section_pad = 7.0 * scale;
    let icon_w = sugarloaf.text_mut().measure(GLYPH_SHELL, &opts);
    let label_w = sugarloaf.text_mut().measure(label, &opts);
    icon_w + label_w + section_pad * 4.0
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_shell_badge(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    pill_y: f32,
    pill_h: f32,
    body_y: f32,
    font_size: f32,
    label: &str,
    kind: TerminalShellKind,
    previous_kind: TerminalShellKind,
    transition_t: f32,
    transition_elapsed_ms: f32,
    animation_phase: f32,
    theme: &IdeTheme,
    scale: f32,
) {
    let section_pad = 7.0 * scale;
    let radius = pill_h / 2.0;
    let eased_t = ease_out_cubic(transition_t);
    let accent = lerp_color_u8(
        shell_badge_accent(previous_kind, theme),
        shell_badge_accent(kind, theme),
        eased_t,
    );
    let text_bg = lerp_color_f32(
        theme.f32(theme.surface),
        color_u8_to_f32(accent),
        0.12 * eased_t,
    );
    let icon_opts = DrawOpts {
        font_size,
        color: theme.u8(theme.black),
        bold: true,
        ..DrawOpts::default()
    };
    let text_opts = DrawOpts {
        font_size,
        color: accent,
        bold: true,
        ..DrawOpts::default()
    };
    let icon_w = sugarloaf.text_mut().measure(GLYPH_SHELL, &icon_opts);
    let label_w = sugarloaf.text_mut().measure(label, &text_opts);
    let icon_section_w = icon_w + section_pad * 2.0;
    let text_section_w = label_w + section_pad * 2.0;
    let pill_w = icon_section_w + text_section_w;

    sugarloaf.rounded_rect(
        None,
        x,
        pill_y,
        pill_w,
        pill_h,
        text_bg,
        DEPTH,
        radius,
        ORDER_CHIP_BG,
    );
    sugarloaf.quad(
        None,
        x,
        pill_y,
        icon_section_w,
        pill_h,
        color_u8_to_f32(accent),
        [radius, 0.0, 0.0, radius],
        DEPTH,
        ORDER_CHIP_BG + 1,
    );
    draw_text_thick(
        sugarloaf,
        x + section_pad,
        body_y,
        GLYPH_SHELL,
        &icon_opts,
        scale,
    );
    let label_x = x + icon_section_w + section_pad;
    if transition_t < 1.0 {
        draw_scramble_text_thick(
            sugarloaf,
            label_x,
            body_y,
            label,
            &text_opts,
            transition_t,
            transition_elapsed_ms,
            animation_phase,
            scale,
        );
    } else {
        draw_text_thick(sugarloaf, label_x, body_y, label, &text_opts, scale);
    }
}
