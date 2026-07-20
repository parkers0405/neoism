use super::*;

use crate::event::{LogicalKey, UiEvent};
use crate::layout::Rect;
use crate::panels::StatusPalette;
use crate::theme::ChromeTheme;
use crate::theme::RgbTriple;

pub(crate) fn rgb_u32(c: RgbTriple) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

pub(crate) fn status_palette_from_theme(theme: &ChromeTheme) -> StatusPalette {
    StatusPalette {
        bg: rgb_u32(theme.bg),
        surface: rgb_u32(theme.bg_elevated),
        muted: rgb_u32(theme.fg_dim),
        red: rgb_u32(theme.error),
        green: rgb_u32(theme.success),
        yellow: rgb_u32(theme.yellow),
        blue: rgb_u32(theme.accent),
        magenta: rgb_u32(theme.magenta),
        cyan: rgb_u32(theme.cyan),
        black: rgb_u32(theme.black),
    }
}

pub(crate) fn pointer_inside(event: &UiEvent, rect: Rect) -> bool {
    match event {
        UiEvent::PointerMove { x, y, .. }
        | UiEvent::PointerDown { x, y, .. }
        | UiEvent::PointerUp { x, y, .. } => rect.contains(*x, *y),
        // Wheel doesn't carry coords in this event vocabulary; treat
        // as inside any rect so the priority-order top still gets to
        // consume it. PointerLeave fans out to everyone for the same
        // reason — panels self-arbitrate by tracking their hover state.
        UiEvent::Wheel { .. } | UiEvent::PointerLeave => true,
        _ => true,
    }
}

pub(crate) fn is_modal_key(key: PanelKey) -> bool {
    matches!(
        key,
        PanelKey::CommandPalette | PanelKey::Finder | PanelKey::GitDiff
    )
}

pub(crate) fn is_character_key(logical: &LogicalKey, needle: &str) -> bool {
    matches!(logical, LogicalKey::Character(ch) if ch.eq_ignore_ascii_case(needle))
}

pub(crate) fn is_colon_or_semicolon_key(logical: &LogicalKey) -> bool {
    matches!(logical, LogicalKey::Character(ch) if ch == ":" || ch == ";")
}
