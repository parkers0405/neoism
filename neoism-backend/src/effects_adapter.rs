//! Native adapter that drains `TerminalEffect`s from `Crosswords` and
//! re-emits them as the historical `RioEvent` variants the desktop
//! app already handles.
//!
//! Phase 2 of the libghostty-style migration: terminal state now
//! produces side effects as plain data (`TerminalEffect`) instead of
//! reaching into a native `EventListener`. The desktop frontend still
//! consumes `RioEvent` — this adapter is the glue.
//!
//! When the daemon / wasm hosts come online they'll drain the same
//! buffer and translate effects into wire messages or JS callbacks
//! without needing this adapter at all.

use std::sync::Arc;

use base64::engine::general_purpose;
use base64::Engine;

use crate::event::{
    EventListener, ProgressReport as BackendProgressReport, RioEvent, WindowId,
};
use neoism_terminal_core::ansi::graphics::UpdateQueues;
use neoism_terminal_core::{TerminalEffect, TextAreaSizeRequestKind};

pub fn dispatch_terminal_effects<U: EventListener>(
    effects: impl IntoIterator<Item = TerminalEffect>,
    event_proxy: &U,
    window_id: WindowId,
    route_id: usize,
) {
    for effect in effects {
        match effect {
            TerminalEffect::PtyWrite(bytes) => {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                event_proxy.send_event(RioEvent::PtyWrite(route_id, text), window_id);
            }
            TerminalEffect::SetTitle(s) => {
                event_proxy.send_event(RioEvent::Title(s), window_id);
            }
            TerminalEffect::ResetTitle => {
                event_proxy.send_event(RioEvent::ResetTitle, window_id);
            }
            TerminalEffect::Bell => {
                event_proxy.send_event(RioEvent::Bell, window_id);
            }
            TerminalEffect::ClipboardStore { ty, text } => {
                event_proxy.send_event(RioEvent::ClipboardStore(ty, text), window_id);
            }
            TerminalEffect::ClipboardLoad {
                ty,
                clipboard_byte,
                terminator,
            } => {
                event_proxy.send_event(
                    RioEvent::ClipboardLoad(
                        route_id,
                        ty,
                        Arc::new(move |content| {
                            let b64 = general_purpose::STANDARD.encode(content);
                            format!(
                                "\x1b]52;{};{}{}",
                                clipboard_byte as char, b64, terminator
                            )
                        }),
                    ),
                    window_id,
                );
            }
            TerminalEffect::DesktopNotification { title, body } => {
                event_proxy
                    .send_event(RioEvent::DesktopNotification { title, body }, window_id);
            }
            TerminalEffect::OpenEditorTab { path } => {
                event_proxy
                    .send_event(RioEvent::OpenEditorTab { route_id, path }, window_id);
            }
            TerminalEffect::ChangeTerminalDirectory { path } => {
                event_proxy.send_event(
                    RioEvent::ChangeTerminalDirectory { route_id, path },
                    window_id,
                );
            }
            TerminalEffect::ColorRequest {
                prefix,
                index,
                terminator,
            } => {
                event_proxy.send_event(
                    RioEvent::ColorRequest(
                        route_id,
                        index,
                        Arc::new(move |color| {
                            format!(
                                "\x1b]{};rgb:{1:02x}{1:02x}/{2:02x}{2:02x}/{3:02x}{3:02x}{4}",
                                prefix, color.r, color.g, color.b, terminator
                            )
                        }),
                    ),
                    window_id,
                );
            }
            TerminalEffect::ColorChange { index, color } => {
                event_proxy
                    .send_event(RioEvent::ColorChange(route_id, index, color), window_id);
            }
            TerminalEffect::TextAreaSizeRequest {
                kind,
                terminator: _,
            } => match kind {
                TextAreaSizeRequestKind::Pixels => {
                    event_proxy.send_event(
                        RioEvent::TextAreaSizeRequest(
                            route_id,
                            Arc::new(|window_size| {
                                format!(
                                    "\x1b[4;{};{}t",
                                    window_size.height, window_size.width
                                )
                            }),
                        ),
                        window_id,
                    );
                }
                TextAreaSizeRequestKind::GraphicsAttribute { pi } => {
                    event_proxy.send_event(
                        RioEvent::TextAreaSizeRequest(
                            route_id,
                            Arc::new(move |window_size| {
                                format!(
                                    "\x1b[?{};0;{};{}S",
                                    pi, window_size.width, window_size.height
                                )
                            }),
                        ),
                        window_id,
                    );
                }
            },
            TerminalEffect::GraphicsUpdate(payload) => {
                if let Ok(queues) = payload.downcast::<UpdateQueues>() {
                    event_proxy.send_event(
                        RioEvent::UpdateGraphics {
                            route_id,
                            queues: *queues,
                        },
                        window_id,
                    );
                }
            }
            TerminalEffect::ProgressReport(report) => {
                event_proxy.send_event(
                    RioEvent::ProgressReport(BackendProgressReport::from(report)),
                    window_id,
                );
            }
            TerminalEffect::CursorBlinkingChange => {
                event_proxy.send_event(RioEvent::CursorBlinkingChange, window_id);
            }
            TerminalEffect::MouseCursorDirty => {
                event_proxy.send_event(RioEvent::MouseCursorDirty, window_id);
            }
            TerminalEffect::Exit => {
                event_proxy.send_event(RioEvent::CloseTerminal(route_id), window_id);
            }
            TerminalEffect::RenderRequest => {
                event_proxy.send_event(RioEvent::RenderRoute(route_id), window_id);
            }
            TerminalEffect::Dirty => {
                // Renderer's existing damage path drives redraws; no
                // separate RioEvent is required for a generic Dirty
                // hint today.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::WindowId;
    use parking_lot::Mutex;
    use std::sync::Arc as StdArc;

    #[derive(Clone, Default)]
    struct RecordingListener {
        events: StdArc<Mutex<Vec<RioEvent>>>,
    }

    impl EventListener for RecordingListener {
        fn event(&self) -> (Option<RioEvent>, bool) {
            (None, false)
        }

        fn send_event(&self, event: RioEvent, _window: WindowId) {
            self.events.lock().push(event);
        }
    }

    #[test]
    fn pty_write_roundtrips_text() {
        let listener = RecordingListener::default();
        dispatch_terminal_effects(
            vec![TerminalEffect::PtyWrite(b"abc".to_vec())],
            &listener,
            WindowId::from(0),
            7,
        );
        let events = listener.events.lock();
        assert_eq!(events.len(), 1);
        match &events[0] {
            RioEvent::PtyWrite(route, text) => {
                assert_eq!(*route, 7);
                assert_eq!(text, "abc");
            }
            other => panic!("expected PtyWrite, got {other:?}"),
        }
    }

    #[test]
    fn bell_maps_to_bell() {
        let listener = RecordingListener::default();
        dispatch_terminal_effects(
            vec![TerminalEffect::Bell],
            &listener,
            WindowId::from(0),
            0,
        );
        assert!(matches!(listener.events.lock()[0], RioEvent::Bell));
    }
}
