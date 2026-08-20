use super::*;

use crate::event::{LogicalKey, UiEvent};
use crate::layout::Rect;

// NOTE: the old `rgb_u32` / `status_palette_from_theme` pair lived here and
// built the status line's palette out of `ChromeTheme`. They are gone on
// purpose: they mapped to different palette slots than the desktop host's
// conversion (`blue<-accent`, `red<-error`, `surface<-bg_elevated`, ...),
// which is exactly why the web status bar painted a different color than
// desktop's. `Chrome::draw` now calls
// `StatusLine::render_with_ide_theme_in_content_bounds` with the real
// `IdeTheme`, the same entry point the native host uses. Don't reintroduce
// a second palette source.

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
