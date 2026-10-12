//! `CommandComposer` core state + constructor + `Default`.
//!
//! Behaviour lives in sibling modules:
//! - `update.rs` — small state mutators and motion helpers
//! - `render.rs` — every-frame paint pass
//! - `completion.rs` — popup geometry + completion list helpers

use web_time::Instant;

use crate::animation::CriticallyDampedSpring;

use super::types::{ComposerFrame, InputWrapLayout, SHELL_TRANSITION_MS};
use crate::input::TerminalShellKind;

pub struct CommandComposer {
    pub(super) visible: bool,
    pub(super) scale: f32,
    /// Wall-clock time of the last render — drives caret blink so the
    /// blink phase is decoupled from terminal animation_phase, which
    /// can stutter when nothing else is asking for redraws.
    pub(super) last_render: Instant,
    pub(super) last_caret_seen: Instant,
    pub(super) last_text: String,
    pub(super) text_viewport: super::text_viewport::TextViewport,
    pub(super) text_line_height: f32,
    pub(super) last_cursor_byte: usize,
    pub(super) last_shell_kind: Option<TerminalShellKind>,
    pub(super) previous_shell_kind: TerminalShellKind,
    pub(super) shell_transition_started: Instant,
    pub(super) completion_popup_started: Instant,
    pub(super) last_completion_count: usize,
    pub(super) last_completion_selected: Option<usize>,
    pub(super) completion_scroll_offset: usize,
    pub(super) completion_scroll_spring: CriticallyDampedSpring,
    pub(super) completion_cursor_spring: CriticallyDampedSpring,
    pub(super) last_completion_scroll_frame: Instant,
    pub(super) last_completion_cursor_frame: Instant,
    pub(super) completion_last_scroll_time: Option<Instant>,
    pub(super) completion_popup_rect: Option<[f32; 4]>,
    pub(super) last_input_wrap: Option<InputWrapLayout>,
    pub(super) last_frame: ComposerFrame,
}

impl CommandComposer {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            visible: true,
            scale: 1.0,
            last_render: now,
            last_caret_seen: now,
            last_text: String::new(),
            text_viewport: Default::default(),
            text_line_height: 0.0,
            last_cursor_byte: 0,
            last_shell_kind: None,
            previous_shell_kind: TerminalShellKind::Unknown,
            shell_transition_started: now
                .checked_sub(web_time::Duration::from_millis(
                    SHELL_TRANSITION_MS as u64 + 1,
                ))
                .unwrap_or(now),
            completion_popup_started: now,
            last_completion_count: 0,
            last_completion_selected: None,
            completion_scroll_offset: 0,
            completion_scroll_spring: CriticallyDampedSpring::new(),
            completion_cursor_spring: CriticallyDampedSpring::new(),
            last_completion_scroll_frame: now,
            last_completion_cursor_frame: now,
            completion_last_scroll_time: None,
            completion_popup_rect: None,
            last_input_wrap: None,
            last_frame: ComposerFrame::default(),
        }
    }
}

impl Default for CommandComposer {
    fn default() -> Self {
        Self::new()
    }
}
